/* Corvid native runtime: refcount verifier.
 *
 * During a GC mark walk we already traverse every reachable edge in
 * the object graph. That traversal can cheaply compute the expected
 * refcount of each reachable block: (roots pointing at it) + (edges
 * from other reachable blocks pointing at it). Diffing that against
 * the actual refcount catches compiler miscompilations in Corvid's
 * ownership optimizer.
 *
 * Modes controlled by CORVID_GC_VERIFY:
 *   off   - verifier never runs; zero cost
 *   warn  - drift printed to stderr, execution continues
 *   abort - drift printed then abort()
 *
 * Storage design:
 *   The verifier uses each allocation's tracking node as per-cycle
 *   scratch storage:
 *   - verify_epoch identifies which GC cycle the scratch data belongs to
 *   - verify_expected_tagged stores expected_rc in the low 63 bits and
 *     a verifier-only visited flag in the high bit
 */

#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

#define CORVID_HEADER_BYTES 16
#define CORVID_REFCOUNT_IMMORTAL ((long long)INT64_MIN)
#define CORVID_RC_MASK 0x1FFFFFFFFFFFFFFFLL
#define CORVID_MARK_BIT (1LL << 61)

typedef struct corvid_typeinfo {
    uint32_t size;
    uint32_t flags;
    void (*destroy_fn)(void*);
    void (*trace_fn)(void*, void (*marker)(void*, void*), void*);
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
    const void* last_retain_pc;
    const void* last_release_pc;
    unsigned long long verify_epoch;
    unsigned long long verify_expected_tagged;
} corvid_tracking_node;

#define CORVID_TRACKING_BYTES (sizeof(corvid_tracking_node))

extern corvid_tracking_node* corvid_live_head;

int corvid_gc_verify_mode = 0;
long long corvid_gc_verify_drift_count = 0;

#define CORVID_VERIFY_VISITED_BIT (1ULL << 63)
#define CORVID_VERIFY_EXPECTED_MASK (~CORVID_VERIFY_VISITED_BIT)

static unsigned long long corvid_verify_epoch = 1;

static corvid_tracking_node* corvid_verify_node_from_payload(void* payload) {
    corvid_header* h = (corvid_header*)((char*)payload - CORVID_HEADER_BYTES);
    return (corvid_tracking_node*)((char*)h - CORVID_TRACKING_BYTES);
}

static void corvid_verify_prepare_node(corvid_tracking_node* node,
                                       unsigned long long epoch) {
    if (node->verify_epoch != epoch) {
        node->verify_epoch = epoch;
        node->verify_expected_tagged = 0;
    }
}

static void corvid_verify_bump_expected(corvid_tracking_node* node,
                                        unsigned long long epoch) {
    corvid_verify_prepare_node(node, epoch);
    unsigned long long expected =
        node->verify_expected_tagged & CORVID_VERIFY_EXPECTED_MASK;
    unsigned long long flags =
        node->verify_expected_tagged & CORVID_VERIFY_VISITED_BIT;
    node->verify_expected_tagged = flags | (expected + 1);
}

static int corvid_verify_mark_visited(corvid_tracking_node* node,
                                      unsigned long long epoch) {
    corvid_verify_prepare_node(node, epoch);
    if ((node->verify_expected_tagged & CORVID_VERIFY_VISITED_BIT) != 0) {
        return 0;
    }
    node->verify_expected_tagged |= CORVID_VERIFY_VISITED_BIT;
    return 1;
}

static uint64_t corvid_verify_expected(corvid_tracking_node* node,
                                       unsigned long long epoch) {
    if (node->verify_epoch != epoch) return 0;
    return node->verify_expected_tagged & CORVID_VERIFY_EXPECTED_MASK;
}

typedef struct {
    unsigned long long epoch;
} verify_ctx;

static void corvid_verify_marker(void* payload, void* ctx_v) {
    if (payload == NULL) return;

    verify_ctx* ctx = (verify_ctx*)ctx_v;
    corvid_tracking_node* node = corvid_verify_node_from_payload(payload);

    corvid_verify_bump_expected(node, ctx->epoch);
    if (!corvid_verify_mark_visited(node, ctx->epoch)) return;

    corvid_header* h = (corvid_header*)((char*)payload - CORVID_HEADER_BYTES);
    if (h->refcount_word == CORVID_REFCOUNT_IMMORTAL) return;
    if (h->typeinfo != NULL && h->typeinfo->trace_fn != NULL) {
        h->typeinfo->trace_fn(payload, corvid_verify_marker, ctx);
    }
}

static void corvid_verify_report(const corvid_header* h,
                                 void* payload,
                                 uint64_t expected,
                                 long long actual_rc,
                                 const corvid_tracking_node* node) {
    corvid_gc_verify_drift_count++;

    const char* ty = (h->typeinfo && h->typeinfo->name)
                         ? h->typeinfo->name
                         : "<unnamed>";
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

void corvid_gc_verify(void** roots, size_t n_roots) {
    if (corvid_gc_verify_mode == 0) return;

    unsigned long long epoch = ++corvid_verify_epoch;
    if (epoch == 0) {
        epoch = 1;
        corvid_verify_epoch = epoch;
        for (corvid_tracking_node* n = corvid_live_head; n != NULL; n = n->next) {
            n->verify_epoch = 0;
            n->verify_expected_tagged = 0;
        }
    }

    verify_ctx ctx = {.epoch = epoch};

    for (size_t i = 0; i < n_roots; i++) {
        if (roots[i] != NULL) {
            corvid_verify_marker(roots[i], &ctx);
        }
    }

    /*
     * In corvid_gc_from_roots mode the explicit root slice already names
     * the full root set, so scanning the live list again would just
     * repeat work. The mark-bit scan is only needed for the stack-walk
     * collector path where the verifier was not given explicit roots.
     */
    if (n_roots == 0) {
        for (corvid_tracking_node* n = corvid_live_head; n != NULL; n = n->next) {
            corvid_header* h =
                (corvid_header*)((char*)n + CORVID_TRACKING_BYTES);
            if (h->refcount_word == CORVID_REFCOUNT_IMMORTAL) continue;
            if ((h->refcount_word & CORVID_MARK_BIT) == 0) continue;

            void* payload = (void*)((char*)h + CORVID_HEADER_BYTES);
            if (!corvid_verify_mark_visited(n, epoch)) continue;

            corvid_verify_bump_expected(n, epoch);
            if (h->typeinfo != NULL && h->typeinfo->trace_fn != NULL) {
                h->typeinfo->trace_fn(payload, corvid_verify_marker, &ctx);
            }
        }
    }

    int abort_requested = 0;
    for (corvid_tracking_node* n = corvid_live_head; n != NULL; n = n->next) {
        corvid_header* h = (corvid_header*)((char*)n + CORVID_TRACKING_BYTES);
        if (h->refcount_word == CORVID_REFCOUNT_IMMORTAL) continue;

        void* payload = (void*)((char*)h + CORVID_HEADER_BYTES);
        uint64_t expected = corvid_verify_expected(n, epoch);
        if (expected == 0) continue;

        long long actual_rc = h->refcount_word & CORVID_RC_MASK;
        if ((uint64_t)actual_rc != expected) {
            corvid_verify_report(h, payload, expected, actual_rc, n);
            if (corvid_gc_verify_mode == 2) abort_requested = 1;
        }
    }

    if (abort_requested) {
        fprintf(stderr, "CORVID_GC_VERIFY=abort: terminating\n");
        abort();
    }
}

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
