/* Corvid native runtime: refcounted heap allocations.
 *
 * Every refcounted Corvid value (String, Struct, List) sits behind a
 * 16-byte header:
 *
 *     [ refcount_word (8) | typeinfo_ptr (8) ]
 *     [ payload bytes...                     ]  <-- alloc returns here
 *
 * Header layout — Phase 17 (slice 17a):
 *
 *   refcount_word (i64), bit-packed:
 *     bits  0..60  refcount
 *     bit   61     mark bit   — 17d cycle collector mark phase
 *     bit   62     color bit  — 17h Bacon-Rajan (interpreter tier)
 *     bit   63     sign       — reserved for INT64_MIN immortal sentinel
 *
 *   typeinfo_ptr (const corvid_typeinfo*):
 *     points at a per-type metadata block emitted in .rodata by the
 *     codegen (or the runtime for built-ins like String).
 *
 * Phase 17d extension — intrusive tracking-node prefix for cycle
 * collection:
 *
 * Heap allocations actually allocate 24 bytes of hidden prefix BEFORE
 * the user-visible 16-byte header:
 *
 *   [ tracking_node (24) ][ refcount_word (8) | typeinfo_ptr (8) ][ payload ]
 *                         ^-- what user/retain/release see as "header"
 *                                              ^-- payload pointer returned
 *
 * The tracking node links every heap allocation into a global
 * doubly-linked list that the sweep phase walks to find unmarked
 * (unreachable) objects. Static-literal strings (in .rodata) have no
 * tracking prefix because they're never collected — the immortal
 * refcount sentinel short-circuits retain/release before any header
 * access, and the collector skips them by design (they're not on
 * g_live_head). User code and codegen see only the 16-byte visible
 * header, so this extension is invisible to `lower_string_literal`
 * et al.
 *
 * Non-atomic refcount: Corvid is single-threaded today. Phase 25 will
 * bring a proper multi-threaded RC design (biased RC, per-arena
 * locks, or deferred RC) — not blanket atomics.
 *
 * Immortal sentinel: static literals (descriptors emitted in .rodata)
 * have refcount = INT64_MIN. retain/release short-circuit on this
 * value. Sentinel sits in bit 63 alone — doesn't overlap with 17d
 * mark bit (61) or 17h color bit (62); GC state on an immortal is
 * unambiguous (immortals are never marked, never collected).
 *
 * Leak detector + RC op counters: four counters track allocs,
 * releases, retain calls, release calls. Printed at exit under
 * CORVID_DEBUG_ALLOC. Parity tests assert ALLOCS == RELEASES.
 */

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <inttypes.h>

#define CORVID_HEADER_BYTES 16

/* Sentinel marking a "never collect" allocation — used by static literals. */
#define CORVID_REFCOUNT_IMMORTAL ((long long)INT64_MIN)

/* Mask isolating the refcount bits from mark/color/sign. */
#define CORVID_RC_MASK 0x1FFFFFFFFFFFFFFFLL

/* Phase 17d — mark bit (bit 61) for the cycle collector's mark phase.
 * Set during mark, cleared at end of sweep. Never touched by user
 * code; retain/release use CORVID_RC_MASK when comparing refcount to
 * avoid clobbering. */
#define CORVID_MARK_BIT (1LL << 61)

/* ---- type-info block ----------------------------------------------- */

typedef struct corvid_typeinfo {
    uint32_t size;
    uint32_t flags;
    void (*destroy_fn)(void*);
    void (*trace_fn)(void*,
                     void (*marker)(void*, void*),
                     void*);
    void (*weak_fn)(void*);
    const struct corvid_typeinfo* elem_typeinfo;
    const char* name;
} corvid_typeinfo;

#define CORVID_TI_CYCLIC_CAPABLE   0x01u
#define CORVID_TI_HAS_WEAK_REFS    0x02u
#define CORVID_TI_IS_LIST          0x04u
#define CORVID_TI_LINEAR_CAPABLE   0x08u
#define CORVID_TI_REGION_ALLOCATABLE 0x10u
#define CORVID_TI_REUSE_SHAPE_HINT 0x20u

typedef struct {
    long long refcount_word;
    const corvid_typeinfo* typeinfo;
} corvid_header;

/* Phase 17d — the hidden tracking node sitting BEFORE the user-visible
 * header for every heap allocation. Links into g_live_head.
 */
typedef struct corvid_tracking_node {
    struct corvid_tracking_node* next;
    struct corvid_tracking_node* prev;
    /* 16-byte header follows (refcount_word + typeinfo_ptr); payload
     * follows that. Total prefix = sizeof(tracking_node) +
     * CORVID_HEADER_BYTES. */
} corvid_tracking_node;

#define CORVID_TRACKING_BYTES (sizeof(corvid_tracking_node))

/* Global head of the live-heap-block list. The sweep phase walks it
 * forward; free unlinks. Exposed (non-static) so collector.c can
 * access directly.
 *
 * Single-threaded access — Phase 25 multi-agent will need a real
 * concurrency story (probably: per-task list merged at safepoints).
 */
corvid_tracking_node* corvid_live_head = NULL;

/* ---- built-in String typeinfo ------------------------------------- */

static void corvid_trace_String_fn(void* payload,
                                   void (*marker)(void*, void*),
                                   void* ctx) {
    (void)payload; (void)marker; (void)ctx;
}

const corvid_typeinfo corvid_typeinfo_String = {
    .size = 0,
    .flags = 0,
    .destroy_fn = NULL,
    .trace_fn = corvid_trace_String_fn,
    .weak_fn = NULL,
    .elem_typeinfo = NULL,
    .name = "String",
};

/* ---- leak detector + RC op counters -------------------------------- */

long long corvid_alloc_count = 0;
long long corvid_release_count = 0;
long long corvid_retain_call_count = 0;
long long corvid_release_call_count = 0;

/* Phase 17d — GC trigger. Allocation-pressure threshold: fire a
 * collection every N allocations. Tunable via `CORVID_GC_TRIGGER`
 * env var (parsed in entry.c's corvid_init, not here — avoids
 * pulling stdlib CRT symbols like strtoll/getenv into the minimal
 * tests that link corvid_c_runtime without a full CRT).
 *
 * 0 disables automatic GC (tests use `corvid_gc()` directly).
 * Default is set by entry.c; if entry.c never ran (test-only
 * linkage), threshold stays 0 = auto-GC off, which is the correct
 * safe default for non-Corvid-main binaries.
 */
long long corvid_gc_trigger_threshold = 0;
static long long corvid_allocs_since_gc = 0;

/* Forward decl — implemented in collector.c. Extern rather than
 * static so `corvid_gc()` is a public C symbol the test + future
 * 17b-7 latency-aware triggers can call directly. */
void corvid_gc(void);

/* ---- runtime API exposed to compiled code -------------------------- */

void* corvid_alloc_typed(long long payload_bytes,
                         const corvid_typeinfo* typeinfo) {
    if (payload_bytes < 0) {
        fprintf(stderr, "corvid: corvid_alloc_typed called with negative size %lld\n",
                payload_bytes);
        exit(1);
    }
    if (typeinfo == NULL) {
        fprintf(stderr, "corvid: corvid_alloc_typed called with NULL typeinfo\n");
        exit(1);
    }
    size_t total = CORVID_TRACKING_BYTES + CORVID_HEADER_BYTES
                   + (size_t)payload_bytes;
    char* raw = (char*)malloc(total);
    if (raw == NULL) {
        fprintf(stderr, "corvid: out of memory (requested %lld bytes)\n",
                payload_bytes);
        exit(1);
    }
    corvid_tracking_node* node = (corvid_tracking_node*)raw;
    corvid_header* h = (corvid_header*)(raw + CORVID_TRACKING_BYTES);

    h->refcount_word = 1;
    h->typeinfo = typeinfo;

    /* Link into the live-block list head (O(1)). */
    node->next = corvid_live_head;
    node->prev = NULL;
    if (corvid_live_head != NULL) {
        corvid_live_head->prev = node;
    }
    corvid_live_head = node;

    corvid_alloc_count++;
    corvid_allocs_since_gc++;

    /* GC trigger: fire at allocation-pressure threshold. Threshold
     * value 0 disables auto-GC (tests explicitly call `corvid_gc()`;
     * binaries without corvid_init never set the threshold, so they
     * also default to no auto-GC). Collector lives in collector.c. */
    if (corvid_gc_trigger_threshold > 0
        && corvid_allocs_since_gc >= corvid_gc_trigger_threshold) {
        corvid_allocs_since_gc = 0;
        corvid_gc();
    }

    return (void*)((char*)h + CORVID_HEADER_BYTES);
}

/* Phase 17d — internal helper used by both corvid_release (when
 * refcount hits 0) and the sweep phase of corvid_gc. Unlinks the
 * tracking node from the live list and frees the combined raw block.
 * The caller is responsible for having run destroy_fn if required.
 */
void corvid_free_block(corvid_header* h) {
    corvid_tracking_node* node = (corvid_tracking_node*)(
        (char*)h - CORVID_TRACKING_BYTES);
    if (node->prev != NULL) {
        node->prev->next = node->next;
    } else {
        corvid_live_head = node->next;
    }
    if (node->next != NULL) {
        node->next->prev = node->prev;
    }
    free((void*)node);
}

void corvid_retain(void* payload) {
    corvid_retain_call_count++;
    if (payload == NULL) return;
    corvid_header* h = (corvid_header*)((char*)payload - CORVID_HEADER_BYTES);
    if (h->refcount_word == CORVID_REFCOUNT_IMMORTAL) return;
    h->refcount_word++;
}

void corvid_release(void* payload) {
    corvid_release_call_count++;
    if (payload == NULL) return;
    corvid_header* h = (corvid_header*)((char*)payload - CORVID_HEADER_BYTES);
    if (h->refcount_word == CORVID_REFCOUNT_IMMORTAL) return;
    long long previous = h->refcount_word;
    h->refcount_word = previous - 1;
    long long prev_rc = previous & CORVID_RC_MASK;
    if (prev_rc == 1) {
        if (h->typeinfo != NULL && h->typeinfo->destroy_fn != NULL) {
            h->typeinfo->destroy_fn(payload);
        }
        corvid_release_count++;
        corvid_free_block(h);
    } else if (prev_rc <= 0) {
        fprintf(stderr,
                "corvid: corvid_release on already-freed allocation (refcount was %lld)\n",
                prev_rc);
        exit(1);
    }
}
