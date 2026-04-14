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
 *     codegen (or the runtime for built-ins like String). The block
 *     holds destroy_fn, trace_fn, size, flags, and (for list types)
 *     elem_typeinfo. `corvid_release` dispatches through destroy_fn;
 *     17d's mark phase dispatches through trace_fn.
 *
 * Non-atomic refcount: Corvid is single-threaded today. The pre-17a
 * design used `_Atomic long long` as future-proofing for Phase 25
 * multi-agent concurrency, paying a LOCK-prefixed RMW on every
 * retain/release forever in exchange for an expected re-migration
 * cost that never actually lands (pre-release project, no binaries
 * in the wild). Phase 25 will bring *proper* multi-threaded RC —
 * biased RC, per-arena locks, or deferred RC — none of which are
 * "sprinkle _Atomic everywhere." Doing non-atomic now buys ~10-50x
 * per-op speedup on hot paths with zero real cost.
 *
 * Immortal sentinel: static literals (descriptors emitted in .rodata)
 * have refcount = INT64_MIN. retain/release short-circuit on this
 * value. The sentinel sits in bit 63 alone — does not overlap with
 * 17d mark bit (61) or 17h color bit (62), so GC state on an
 * immortal is unambiguous.
 *
 * Leak detector: two counters track total allocations and total
 * releases. The shim prints them on exit when CORVID_DEBUG_ALLOC is
 * set. Parity tests assert the two are equal.
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

/* ---- type-info block ----------------------------------------------- */

typedef struct corvid_typeinfo {
    uint32_t size;                     /* payload size hint; 0 = variable */
    uint32_t flags;                    /* CORVID_TI_* bits below */
    void (*destroy_fn)(void*);         /* release refcounted children; NULL if none */
    void (*trace_fn)(void*,
                     void (*marker)(void*, void*),
                     void*);
    void (*weak_fn)(void*);            /* reserved for 17g Weak<T>; NULL in 17a */
    const struct corvid_typeinfo* elem_typeinfo; /* list element type; NULL for non-lists */
    const char* name;                  /* type name, for debugging */
} corvid_typeinfo;

/* flags bits — mirrored in corvid-codegen-cl/src/lowering.rs. */
#define CORVID_TI_CYCLIC_CAPABLE   0x01u  /* can participate in cycles (17e) */
#define CORVID_TI_HAS_WEAK_REFS    0x02u  /* at least one Weak<T> field (17g) */
#define CORVID_TI_IS_LIST          0x04u  /* list type — walker uses elem_typeinfo */
#define CORVID_TI_LINEAR_CAPABLE   0x08u  /* Perceus can elide RC ops (17b-prime) */
#define CORVID_TI_REGION_ALLOCATABLE 0x10u /* escape analysis can arena-allocate (17b-prime) */
#define CORVID_TI_REUSE_SHAPE_HINT 0x20u  /* shape compatible with in-place reuse (17b-prime) */

typedef struct {
    long long refcount_word;           /* packed: see header comment */
    const corvid_typeinfo* typeinfo;
} corvid_header;

/* Built-in String typeinfo — lives with the runtime so string-less
 * programs don't pay for a codegen-emitted block. Leaf type: empty
 * trace, no destroy_fn, no elem_typeinfo. */

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

/* ---- leak detector counters ---------------------------------------- */

long long corvid_alloc_count = 0;
long long corvid_release_count = 0;

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
    char* block = (char*)malloc(CORVID_HEADER_BYTES + (size_t)payload_bytes);
    if (block == NULL) {
        fprintf(stderr, "corvid: out of memory (requested %lld bytes)\n",
                payload_bytes);
        exit(1);
    }
    corvid_header* h = (corvid_header*)block;
    h->refcount_word = 1;
    h->typeinfo = typeinfo;
    corvid_alloc_count++;
    return (void*)(block + CORVID_HEADER_BYTES);
}

void corvid_retain(void* payload) {
    if (payload == NULL) return;
    corvid_header* h = (corvid_header*)((char*)payload - CORVID_HEADER_BYTES);
    if (h->refcount_word == CORVID_REFCOUNT_IMMORTAL) return;
    h->refcount_word++;
}

void corvid_release(void* payload) {
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
        free((void*)h);
    } else if (prev_rc <= 0) {
        fprintf(stderr,
                "corvid: corvid_release on already-freed allocation (refcount was %lld)\n",
                prev_rc);
        exit(1);
    }
}
