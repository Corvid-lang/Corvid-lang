/* Corvid native runtime: refcount verifier (Phase 17f++).
 *
 * During a GC mark pass we already traverse every reachable edge in
 * the object graph. That traversal can cheaply compute the EXPECTED
 * refcount of each reachable block: (roots pointing at it) + (edges
 * from other reachable blocks pointing at it). Diffing that against
 * the actual refcount catches compiler miscompilations in Corvid's
 * ownership optimizer (17b) — specifically:
 *
 *   - missing retain  (under-count  → actual < expected, UAF risk)
 *   - missing release (over-count   → actual > expected, leak)
 *   - duplicate retain/release in paired sites
 *
 * Modes controlled by CORVID_GC_VERIFY:
 *   off    (default) — verifier never runs; zero cost
 *   warn            — drift printed to stderr, execution continues
 *   abort           — drift printed then abort()
 *
 * Storage design:
 *   During the mark walk we accumulate expected counts into a small
 *   open-addressed hash map keyed by block address. The map is
 *   allocated once per GC cycle and freed before the cycle returns.
 *   Map sizing: 2× the live-block count (rounded up to power of two),
 *   so load factor <= 0.5, so probe chains stay short.
 *
 *   For programs with N live blocks the added work is:
 *     - one map insert per edge traversal  (O(edges))
 *     - one final walk of the live list to diff  (O(N))
 *   Mark phase is already O(edges); verifier is the same complexity,
 *   not asymptotically worse.
 *
 * Blame report:
 *   Each tracking node carries last_retain_pc and last_release_pc,
 *   stamped by retain/release via compiler return-address intrinsics.
 *   When drift is reported the PCs are printed so the user can trace
 *   the last RC op back to source via the stack map or symbol table.
 *
 * Non-goals:
 *   - Per-edge blame. We report the BLOCK whose count drifted, not
 *     the specific edge that was miscounted. That's sufficient — the
 *     last_retain/release PC localizes the bug.
 *   - Concurrent access. Corvid is single-threaded; Phase 25 will
 *     revisit.
 *   - Catching temporary drift. Between retain/release pairs the
 *     count is supposed to move. The verifier only fires during GC,
 *     which runs at safepoints where the graph is quiescent.
 */

#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* ---- types (must match alloc.c / collector.c layout) ---- */

#define CORVID_HEADER_BYTES 16
#define CORVID_REFCOUNT_IMMORTAL ((long long)INT64_MIN)
#define CORVID_RC_MASK           0x1FFFFFFFFFFFFFFFLL
#define CORVID_MARK_BIT          (1LL << 61)

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

typedef struct {
    long long refcount_word;
    const corvid_typeinfo* typeinfo;
} corvid_header;

typedef struct corvid_tracking_node {
    struct corvid_tracking_node* next;
    struct corvid_tracking_node* prev;
    /* Phase 17f++ blame fields. Stamped by retain/release via
     * return-address intrinsics; read here when drift is reported. */
    const void* last_retain_pc;
    const void* last_release_pc;
} corvid_tracking_node;

#define CORVID_TRACKING_BYTES (sizeof(corvid_tracking_node))

extern corvid_tracking_node* corvid_live_head;

/* ---- verifier mode + public API ---- */

/* Set by entry.c's corvid_init based on CORVID_GC_VERIFY env var.
 * 0 = off (fast path), 1 = warn, 2 = abort. */
int corvid_gc_verify_mode = 0;

/* Count of drift reports emitted this process. Useful for CI: assert
 * this stays 0 at exit. Exposed as a C symbol for tests. */
long long corvid_gc_verify_drift_count = 0;

/* ---- shadow-count hash map (open addressing, linear probe) ---- */

typedef struct {
    void* key;            /* payload pointer, or NULL for empty slot */
    uint64_t expected_rc; /* accumulated expected refcount */
} shadow_slot;

typedef struct {
    shadow_slot* slots;
    size_t capacity;   /* power of two */
    size_t mask;       /* capacity - 1 */
} shadow_map;

/* Pointer-mixing hash. xorshift on the raw pointer bits — not a
 * cryptographic hash, but good enough for open addressing when
 * capacity is a power of two. */
static size_t shadow_hash(void* p) {
    uintptr_t x = (uintptr_t)p;
    x ^= x >> 33;
    x *= (uintptr_t)0xff51afd7ed558ccdULL;
    x ^= x >> 33;
    x *= (uintptr_t)0xc4ceb9fe1a85ec53ULL;
    x ^= x >> 33;
    return (size_t)x;
}

static int shadow_map_init(shadow_map* m, size_t live_count) {
    size_t cap = 16;
    while (cap < live_count * 2) cap <<= 1;
    m->slots = (shadow_slot*)calloc(cap, sizeof(shadow_slot));
    if (m->slots == NULL) return 0;
    m->capacity = cap;
    m->mask = cap - 1;
    return 1;
}

static void shadow_map_free(shadow_map* m) {
    free(m->slots);
    m->slots = NULL;
    m->capacity = 0;
    m->mask = 0;
}

/* Record an incoming edge to `payload`. Called once per reachable
 * edge during the verifier's mark traversal. */
static void shadow_map_bump(shadow_map* m, void* payload) {
    if (payload == NULL) return;
    size_t i = shadow_hash(payload) & m->mask;
    for (;;) {
        if (m->slots[i].key == NULL) {
            m->slots[i].key = payload;
            m->slots[i].expected_rc = 1;
            return;
        }
        if (m->slots[i].key == payload) {
            m->slots[i].expected_rc++;
            return;
        }
        i = (i + 1) & m->mask;
    }
}

/* Fetch expected count for a block, or 0 if never seen (= unreachable). */
static uint64_t shadow_map_get(shadow_map* m, void* payload) {
    size_t i = shadow_hash(payload) & m->mask;
    for (;;) {
        if (m->slots[i].key == NULL) return 0;
        if (m->slots[i].key == payload) return m->slots[i].expected_rc;
        i = (i + 1) & m->mask;
    }
}

/* ---- verifier mark traversal ---- */

/* We can't reuse the collector's mark walk directly because that one
 * sets the mark bit (destructive for its own re-entry check) and
 * doesn't tally incoming edges. The verifier runs its own parallel
 * traversal using a "visited" set encoded in a bit of the refcount
 * word we don't otherwise use.
 *
 * Bit 60 of refcount_word is the VISIT bit — set while the verifier
 * recurses, cleared at end. It sits inside CORVID_RC_MASK so we must
 * mask it out when reading refcount and restore it atomically. For
 * correctness we only set/clear it within this single-threaded pass
 * and assert it's zero on entry.
 *
 * Wait — bit 60 IS part of the count space per alloc.c's comment.
 * Re-reading: "bits 0..60 refcount, bit 61 mark, bit 62 color, bit 63
 * sign". So bit 60 is used by the count. Squeezing a visit bit out of
 * the count space is incorrect.
 *
 * Instead: we use a SECOND open-addressed map to track "visited" —
 * same hash/probe strategy. Small cost, cleanly separated. */

typedef struct {
    void** slots;
    size_t capacity;
    size_t mask;
} visited_set;

static int visited_set_init(visited_set* v, size_t live_count) {
    size_t cap = 16;
    while (cap < live_count * 2) cap <<= 1;
    v->slots = (void**)calloc(cap, sizeof(void*));
    if (v->slots == NULL) return 0;
    v->capacity = cap;
    v->mask = cap - 1;
    return 1;
}

static void visited_set_free(visited_set* v) {
    free(v->slots);
    v->slots = NULL;
}

/* Insert payload; return 1 if newly inserted, 0 if already present. */
static int visited_set_add(visited_set* v, void* payload) {
    size_t i = shadow_hash(payload) & v->mask;
    for (;;) {
        if (v->slots[i] == NULL) { v->slots[i] = payload; return 1; }
        if (v->slots[i] == payload) return 0;
        i = (i + 1) & v->mask;
    }
}

/* Marker context passed through trace_fn during verifier traversal.
 * Bundles the shadow map + visited set so a single pointer threads
 * them through the C callback signature. */
typedef struct {
    shadow_map* shadow;
    visited_set* visited;
} verify_ctx;

static void corvid_verify_marker(void* payload, void* ctx_v) {
    if (payload == NULL) return;
    verify_ctx* ctx = (verify_ctx*)ctx_v;

    /* Record incoming edge regardless of visit state — an edge is an
     * edge even if we've already recursed into its target. */
    shadow_map_bump(ctx->shadow, payload);

    /* Recurse only if this is the first visit. */
    if (!visited_set_add(ctx->visited, payload)) return;

    corvid_header* h = (corvid_header*)((char*)payload - CORVID_HEADER_BYTES);
    if (h->refcount_word == CORVID_REFCOUNT_IMMORTAL) return;
    if (h->typeinfo != NULL && h->typeinfo->trace_fn != NULL) {
        h->typeinfo->trace_fn(payload, corvid_verify_marker, ctx);
    }
}

/* ---- drift reporting ---- */

static void corvid_verify_report(const corvid_header* h,
                                 void* payload,
                                 uint64_t expected,
                                 long long actual_rc,
                                 const corvid_tracking_node* node) {
    corvid_gc_verify_drift_count++;
    const char* ty = (h->typeinfo && h->typeinfo->name)
        ? h->typeinfo->name : "<unnamed>";
    const char* kind;
    if ((uint64_t)actual_rc < expected) {
        kind = "under-count (missing retain; UAF risk)";
    } else {
        kind = "over-count (missing release; leak)";
    }
    fprintf(stderr,
            "CORVID_GC_VERIFY: refcount drift\n"
            "  block:          %p typeinfo=%s\n"
            "  expected_rc:    %llu\n"
            "  actual_rc:      %lld\n"
            "  diagnosis:      %s\n"
            "  last_retain_pc: %p\n"
            "  last_release_pc:%p\n",
            payload,
            ty,
            (unsigned long long)expected,
            actual_rc,
            kind,
            node->last_retain_pc,
            node->last_release_pc);
}

/* ---- public entry (called from collector.c) ---- */

/* Run the verifier over the given explicit roots + the GC-mark-set
 * the collector is about to sweep. Roots are user-supplied when
 * verifier is called from `corvid_gc_from_roots`. For the stack-walk
 * variant we pass the already-marked set by scanning blocks that
 * carry the mark bit: each already-marked block is a reachable
 * "root" from the verifier's perspective, and we re-traverse from
 * those to accumulate expected counts.
 *
 * The re-traversal is deliberate: we don't want to couple verifier
 * correctness to the collector's internal marker state. A clean
 * second pass over the same reachable set is cheap and keeps the
 * two components independent.
 */
void corvid_gc_verify(void** roots, size_t n_roots) {
    if (corvid_gc_verify_mode == 0) return;

    /* Count live blocks to size the maps. */
    size_t live_count = 0;
    for (corvid_tracking_node* n = corvid_live_head; n != NULL; n = n->next) {
        live_count++;
    }

    shadow_map shadow;
    visited_set visited;
    if (!shadow_map_init(&shadow, live_count)) return;
    if (!visited_set_init(&visited, live_count)) {
        shadow_map_free(&shadow);
        return;
    }
    verify_ctx ctx = { .shadow = &shadow, .visited = &visited };

    /* Walk from explicit roots. */
    for (size_t i = 0; i < n_roots; i++) {
        if (roots[i] != NULL) {
            corvid_verify_marker(roots[i], &ctx);
        }
    }

    /* Also walk from blocks the collector already marked (stack-
     * walk case). We haven't been handed those explicitly, so we
     * scan the live list for mark-bit-set blocks that we haven't
     * visited yet, and use them as additional roots.
     *
     * Note: a stack-rooted block will have been recorded as an
     * incoming edge by its traversal (bumping shadow count). That's
     * correct — the stack IS an edge. The diff step below compares
     * total edges (stack + heap) to refcount, which is exactly the
     * invariant refcount encodes. */
    for (corvid_tracking_node* n = corvid_live_head; n != NULL; n = n->next) {
        corvid_header* h = (corvid_header*)((char*)n + CORVID_TRACKING_BYTES);
        if (h->refcount_word == CORVID_REFCOUNT_IMMORTAL) continue;
        if ((h->refcount_word & CORVID_MARK_BIT) == 0) continue;
        void* payload = (void*)((char*)h + CORVID_HEADER_BYTES);
        if (!visited_set_add(&visited, payload)) continue;

        /* Stack-rooted: count it as one incoming edge. */
        shadow_map_bump(&shadow, payload);

        if (h->typeinfo != NULL && h->typeinfo->trace_fn != NULL) {
            h->typeinfo->trace_fn(payload, corvid_verify_marker, &ctx);
        }
    }

    /* Diff: walk every reachable block, compare expected vs actual. */
    int abort_requested = 0;
    for (corvid_tracking_node* n = corvid_live_head; n != NULL; n = n->next) {
        corvid_header* h = (corvid_header*)((char*)n + CORVID_TRACKING_BYTES);
        if (h->refcount_word == CORVID_REFCOUNT_IMMORTAL) continue;
        void* payload = (void*)((char*)h + CORVID_HEADER_BYTES);
        uint64_t expected = shadow_map_get(&shadow, payload);
        if (expected == 0) continue; /* unreachable; sweep will free */

        long long actual_rc = h->refcount_word & CORVID_RC_MASK;
        if ((uint64_t)actual_rc != expected) {
            corvid_verify_report(h, payload, expected, actual_rc, n);
            if (corvid_gc_verify_mode == 2) abort_requested = 1;
        }
    }

    shadow_map_free(&shadow);
    visited_set_free(&visited);

    if (abort_requested) {
        fprintf(stderr, "CORVID_GC_VERIFY=abort: terminating\n");
        abort();
    }
}

/* Test-only helper: deliberately corrupt a refcount to exercise the
 * verifier. Exposed so Rust integration tests can assert the
 * verifier actually detects drift it's supposed to detect. Not a
 * stable API. */
void corvid_verify_corrupt_rc(void* payload, long long delta) {
    if (payload == NULL) return;
    corvid_header* h = (corvid_header*)((char*)payload - CORVID_HEADER_BYTES);
    if (h->refcount_word == CORVID_REFCOUNT_IMMORTAL) return;
    long long rc = h->refcount_word & CORVID_RC_MASK;
    long long high = h->refcount_word & ~CORVID_RC_MASK;
    long long new_rc = rc + delta;
    if (new_rc < 0) new_rc = 0;
    if (new_rc > CORVID_RC_MASK) new_rc = CORVID_RC_MASK;
    h->refcount_word = high | new_rc;
}
