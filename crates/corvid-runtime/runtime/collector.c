/* Corvid native runtime: cycle collector (Phase 17d).
 *
 * Mark-sweep cycle collector over the refcount heap. Fires when
 * allocation pressure crosses the threshold set by `CORVID_GC_TRIGGER`
 * (default 10_000 allocations), or when user/test code calls
 * `corvid_gc()` directly.
 *
 * Complements the refcount fast path: acyclic data is freed
 * deterministically by `corvid_release` as soon as refcount hits 0.
 * The collector exists only to catch cycles — refs that form a loop
 * and keep each other alive even though no external reference exists.
 *
 * ## Phase coordination
 *
 * - 17a: heap header with typeinfo pointer + mark bit slot in
 *   refcount word. Typeinfo has `trace_fn(payload, marker, ctx)` that
 *   calls `marker` on every refcounted child pointer. Load-bearing.
 * - 17c: `corvid_stack_maps` symbol + `corvid_stack_maps_find(pc)`.
 *   Load-bearing.
 * - Cranelift `preserve_frame_pointers` flag: emits RBP-chained
 *   frames so the mark phase can walk the stack without OS-specific
 *   unwind info. Set in `corvid-codegen-cl/src/module.rs`.
 * - 17d (this file): mark + sweep.
 *
 * ## Algorithm
 *
 * MARK:
 *   1. Capture current RBP.
 *   2. Walk RBP chain: for each frame, read return PC from [rbp+8],
 *      compute SP-at-call = rbp + 16, look up return PC in
 *      `corvid_stack_maps`. If found, for each `ref_offsets[i]` in
 *      the entry, read the pointer at `sp_at_call + offset` and mark
 *      its referent.
 *   3. Marking = set mark bit + recurse via `typeinfo.trace_fn`.
 *      Skip if already marked (cycle-safe) or immortal.
 *
 * SWEEP:
 *   1. Walk `corvid_live_head` list. For each unmarked block:
 *      a. Trace its refs with `trace_fn`, calling a DECREMENT marker
 *         on each (not free — just drop the refcount). This avoids
 *         leaking refcount on marked children that this unreachable
 *         block referenced.
 *      b. Free the block raw (no destroy_fn — we've already
 *         decremented children via the trace walk).
 *   2. Walk the list again, clearing mark bit on each surviving
 *      (marked) block. Ready for the next cycle.
 *
 * Correctness argument for the two-pass sweep:
 *   - Unmarked blocks are unreachable (mark phase found every root
 *     and traced every live pointer). Cycles' members are unmarked.
 *   - Decrementing refs via trace_fn before free means refcount
 *     bookkeeping stays consistent: any marked child referenced from
 *     an unreachable block gets its count correctly reduced to its
 *     externally-reachable count. Any unreachable-child reference
 *     reduces the child's count but we free it anyway in this same
 *     sweep.
 *   - The decrement pass cannot call corvid_release on a child that
 *     was already freed in the sweep, because we decrement ALL
 *     unreachable blocks' refs BEFORE freeing any. Two passes:
 *     (A) decrement all unreachable refs; (B) free all unreachable
 *     blocks. Currently implemented as a single combined pass that
 *     uses `corvid_free_block` (no destroy_fn, no recursive release)
 *     — the decrement-marker callback is in-memory-only and can't
 *     trigger free even if a child's count hits 0, because the
 *     callback doesn't check for zero.
 *
 * Single-threaded: no atomic ops. Mutator is paused (by calling-
 * convention: we're called from within alloc or from user code;
 * there's no other thread running Corvid values in v0.1).
 */

#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

#if defined(_MSC_VER)
/* MSVC intrinsic to get the address of the current function's
 * return address. Subtracting 8 gives the saved RBP address on
 * x64 Windows. */
#pragma intrinsic(_AddressOfReturnAddress)
void* _AddressOfReturnAddress(void);
#endif

/* ---- types (must match alloc.c exactly) ---------------------------- */

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
    const void* last_retain_pc;
    const void* last_release_pc;
} corvid_tracking_node;

#define CORVID_TRACKING_BYTES (sizeof(corvid_tracking_node))

extern corvid_tracking_node* corvid_live_head;
extern long long corvid_release_count;

extern void corvid_free_block(corvid_header* h);

/* Must match the layout declared in stack_maps.c. */
typedef struct corvid_stack_map_entry {
    const void* fn_start;
    uint32_t pc_offset;
    uint32_t frame_bytes;
    uint32_t ref_count;
    uint32_t _pad;
    const uint32_t* ref_offsets;
} corvid_stack_map_entry;

extern const corvid_stack_map_entry*
corvid_stack_maps_find(const void* return_pc);

/* Phase 17f++ verifier + replay-determinism hooks (verify.c). */
extern int corvid_gc_verify_mode;
extern void corvid_gc_verify(void** roots, size_t n_roots);

/* Phase 17f++ — trigger-point log. Every GC cycle appends one entry
 * so record/replay can reproduce trigger points across runs even if
 * the optimizer changes allocation patterns. Replay-side consumes
 * this under Phase 19. For now: record only, readable via the
 * `corvid_gc_trigger_log_*` accessors exposed below. */
typedef struct {
    long long alloc_count;
    long long safepoint_count;
    long long cycle_index;
} corvid_gc_trigger_record;

extern long long corvid_alloc_count;
extern long long corvid_safepoint_count;

#define CORVID_GC_TRIGGER_LOG_CAP 1024
static corvid_gc_trigger_record corvid_gc_trigger_log[CORVID_GC_TRIGGER_LOG_CAP];
static long long corvid_gc_trigger_log_len = 0;
static long long corvid_gc_cycle_count = 0;

/* C-visible accessors. Tests + Phase 19 replay infrastructure read
 * through these rather than touching the static array directly. */
long long corvid_gc_trigger_log_length(void) {
    return corvid_gc_trigger_log_len;
}

int corvid_gc_trigger_log_at(long long index,
                             long long* out_alloc,
                             long long* out_safepoint,
                             long long* out_cycle) {
    if (index < 0 || index >= corvid_gc_trigger_log_len) return 0;
    const corvid_gc_trigger_record* r = &corvid_gc_trigger_log[index];
    if (out_alloc) *out_alloc = r->alloc_count;
    if (out_safepoint) *out_safepoint = r->safepoint_count;
    if (out_cycle) *out_cycle = r->cycle_index;
    return 1;
}

static void corvid_gc_record_trigger(void) {
    if (corvid_gc_trigger_log_len >= CORVID_GC_TRIGGER_LOG_CAP) return;
    corvid_gc_trigger_record* r =
        &corvid_gc_trigger_log[corvid_gc_trigger_log_len++];
    r->alloc_count = corvid_alloc_count;
    r->safepoint_count = corvid_safepoint_count;
    r->cycle_index = corvid_gc_cycle_count++;
}

/* ---- frame-pointer walk -------------------------------------------- */

/* Capture the caller's RBP. Returns what this function's RBP points
 * at — i.e. the caller's saved RBP. Walking from here traverses the
 * call chain toward main.
 *
 * The Cranelift flag `preserve_frame_pointers` guarantees every
 * Corvid-compiled function emits the standard prologue
 * (`push rbp; mov rbp, rsp`), so every frame in the chain has a
 * valid `[rbp+0]=prev_rbp, rbp+8=return_pc` layout on x64.
 * C-runtime frames (malloc, libc, etc.) honor the same ABI in all
 * supported toolchains. Non-x64 targets would need this rewritten;
 * Corvid's ISA today is x64 only.
 */
static void* corvid_gc_capture_rbp(void) {
#if defined(_MSC_VER)
    /* On MSVC x64: _AddressOfReturnAddress() returns RSP at call
     * entry. After `push rbp; mov rbp, rsp` in the prologue, the
     * saved RBP sits at RSP-8 = (&return_addr) - 8. */
    return (char*)_AddressOfReturnAddress() - 8;
#else
    /* GCC/Clang: builtin gives us the current function's RBP
     * directly. */
    return __builtin_frame_address(0);
#endif
}

/* ---- mark phase ---------------------------------------------------- */

/* Set `h->refcount_word`'s mark bit. Returns 1 if this was the
 * transition from unmarked to marked (caller should recurse), 0 if
 * already marked (caller should stop to avoid infinite recursion on
 * cycles). Immortal objects short-circuit: no mark bit ever set.
 */
static int corvid_mark_block(corvid_header* h) {
    if (h->refcount_word == CORVID_REFCOUNT_IMMORTAL) return 0;
    if ((h->refcount_word & CORVID_MARK_BIT) != 0) return 0;
    h->refcount_word |= CORVID_MARK_BIT;
    return 1;
}

/* The marker callback passed into `trace_fn`. Sets the mark bit and
 * recursively traces the child's children if it was newly marked.
 * `ctx` is unused for 17d — reserved for 17b-7 latency-aware RC
 * extensions.
 */
static void corvid_gc_mark_marker(void* payload, void* ctx) {
    (void)ctx;
    if (payload == NULL) return;
    corvid_header* h = (corvid_header*)((char*)payload - CORVID_HEADER_BYTES);
    if (!corvid_mark_block(h)) return;
    if (h->typeinfo != NULL && h->typeinfo->trace_fn != NULL) {
        h->typeinfo->trace_fn(payload, corvid_gc_mark_marker, NULL);
    }
}

/* Walk the RBP chain, consult `corvid_stack_maps` for each frame
 * whose return PC matches a safepoint, mark every live refcounted
 * pointer at the recorded SP-relative offsets.
 *
 * Safety:
 * - `base_rbp` must be an RBP from a live frame on the current
 *   stack. We get it from `corvid_gc_capture_rbp()` immediately
 *   inside `corvid_gc`, so the chain is rooted in a real frame.
 * - We terminate when `frame` is NULL or clearly invalid (non-
 *   8-aligned, below the previous frame — reversed chains signal
 *   corrupted stacks or the top of the thread's stack).
 */
static void corvid_gc_mark_stack(void* base_rbp) {
    void** frame = (void**)base_rbp;
    void** prev_frame = NULL;

    /* Defense-in-depth for walking stacks of binaries that may not
     * preserve frame pointers uniformly (Rust test binaries in
     * particular). Cranelift's `preserve_frame_pointers` flag
     * guarantees Corvid-compiled frames participate; C runtime
     * frames generally participate too. But when this is invoked
     * from Rust code (e.g., integration tests calling corvid_gc
     * directly), the caller's saved RBP may be garbage.
     *
     *   - FRAME_LIMIT: cap absolute number of walked frames.
     *   - STACK_RANGE_BYTES: stop if we walk more than this many
     *     bytes away from base_rbp in either direction. Typical
     *     stacks are <= 1 MB on Windows, 8 MB on Linux; this
     *     bounds the walk to a sane range without needing
     *     OS-specific stack-limit APIs.
     *   - Alignment + monotonicity checks reject garbage RBPs.
     *   - return_pc < 0x1000 is garbage on every modern OS.
     *
     * When all checks pass we still dereference `frame[0..1]`.
     * If that dereference hits unmapped memory we AV — the
     * bounded range makes this rare but not impossible. Tests
     * that need deterministic behavior should use
     * `corvid_gc_from_roots` instead.
     */
    const int FRAME_LIMIT = 256;
    const uintptr_t STACK_RANGE_BYTES = 2 * 1024 * 1024; /* 2 MB */
    const uintptr_t base_addr = (uintptr_t)base_rbp;
    int frame_count = 0;

    while (frame != NULL && frame_count < FRAME_LIMIT) {
        frame_count++;
        if (((uintptr_t)frame & 0x7) != 0) break;
        if (prev_frame != NULL && frame <= prev_frame) break;

        uintptr_t diff = (uintptr_t)frame > base_addr
            ? (uintptr_t)frame - base_addr
            : base_addr - (uintptr_t)frame;
        if (diff > STACK_RANGE_BYTES) break;

        void* return_pc = frame[1];
        void* saved_rbp = frame[0];

        /* Obvious-garbage skip: tiny PC values (< 0x1000) are
         * almost certainly not real code addresses. Modern OSes
         * don't map the first page. */
        if ((uintptr_t)return_pc < 0x1000) break;

        /* Compute SP at the call site: the caller's SP after the
         * call instruction pushed return_pc and our prologue pushed
         * the saved RBP is `frame` itself. But Cranelift's stack
         * map offsets are relative to the SP value AT the call
         * instruction — which is `frame + 16` (past saved RBP and
         * return addr, which are BELOW the caller's stack frame).
         *
         * Actually: the stack map entry for return_pc describes
         * the CALLER's frame at the point of the call. At the call
         * instruction, the caller had its own SP. When we push
         * return_pc (by the `call` op) + saved rbp (by our prologue),
         * we've grown the stack by 16 bytes. So caller's SP-at-call
         * = our current RBP + 16. */
        void* sp_at_call = (char*)frame + 16;

        const corvid_stack_map_entry* e =
            corvid_stack_maps_find(return_pc);
        if (e != NULL) {
            for (uint32_t i = 0; i < e->ref_count; i++) {
                uint32_t offset = e->ref_offsets[i];
                void** slot = (void**)((char*)sp_at_call + offset);
                void* gc_ref = *slot;
                if (gc_ref != NULL) {
                    corvid_gc_mark_marker(gc_ref, NULL);
                }
            }
        }

        prev_frame = frame;
        frame = (void**)saved_rbp;
    }
}

/* ---- sweep phase --------------------------------------------------- */

/* Decrement-only marker used during sweep of unreachable blocks.
 * Sees a child pointer, drops its refcount by 1 (respecting
 * immortality and mark-bit preservation). Does NOT free even if the
 * refcount hits 0 — sweep's second pass handles memory freeing for
 * all unreachable blocks together.
 */
static void corvid_gc_decrement_marker(void* payload, void* ctx) {
    (void)ctx;
    if (payload == NULL) return;
    corvid_header* h = (corvid_header*)((char*)payload - CORVID_HEADER_BYTES);
    if (h->refcount_word == CORVID_REFCOUNT_IMMORTAL) return;
    /* Subtract 1 from the refcount bits without touching mark/color
     * bits. In practice the word's low bits hold the count so a
     * plain -- would work today (mark bit is bit 61, never crosses
     * bits 0-60 during normal ref changes). Using explicit masked
     * arithmetic keeps the code correct if bit layout changes. */
    long long rc = h->refcount_word & CORVID_RC_MASK;
    long long high = h->refcount_word & ~CORVID_RC_MASK;
    if (rc > 0) {
        h->refcount_word = high | (rc - 1);
    }
}

static void corvid_gc_sweep(void) {
    /* Pass 1: trace every unmarked block's children with the
     * decrement-only marker. This keeps refcount bookkeeping
     * consistent for any marked (reachable) children that
     * unreachable blocks referenced. */
    corvid_tracking_node* node = corvid_live_head;
    while (node != NULL) {
        corvid_header* h = (corvid_header*)((char*)node + CORVID_TRACKING_BYTES);
        if (h->refcount_word != CORVID_REFCOUNT_IMMORTAL
            && (h->refcount_word & CORVID_MARK_BIT) == 0) {
            void* payload = (void*)((char*)h + CORVID_HEADER_BYTES);
            if (h->typeinfo != NULL && h->typeinfo->trace_fn != NULL) {
                h->typeinfo->trace_fn(payload, corvid_gc_decrement_marker, NULL);
            }
        }
        node = node->next;
    }

    /* Pass 2: free every unmarked block. Walk with a next-pointer
     * snapshot since corvid_free_block unlinks from the list we're
     * walking. Also clear mark bits on surviving (marked) blocks —
     * ready for the next cycle. */
    node = corvid_live_head;
    while (node != NULL) {
        corvid_tracking_node* next = node->next;
        corvid_header* h = (corvid_header*)((char*)node + CORVID_TRACKING_BYTES);
        if (h->refcount_word != CORVID_REFCOUNT_IMMORTAL) {
            if ((h->refcount_word & CORVID_MARK_BIT) == 0) {
                /* Unreachable. No destroy_fn call — its children
                 * were already decremented in pass 1, and any
                 * unmarked children are in this same sweep. */
                corvid_release_count++;
                corvid_free_block(h);
            } else {
                /* Reachable. Clear mark bit for next cycle. */
                h->refcount_word &= ~CORVID_MARK_BIT;
            }
        }
        node = next;
    }
}

/* ---- public entry -------------------------------------------------- */

/* Run a full mark-sweep cycle. Safe to call at any point that's not
 * already inside GC. Called from `corvid_alloc_typed` when the
 * allocation-pressure threshold fires, and may be called directly by
 * tests via the `corvid_gc` C symbol.
 *
 * Guarded by a re-entrancy flag: if a `trace_fn` itself triggers an
 * allocation (unusual but possible if a typeinfo's trace path
 * constructs temporary values), we skip the nested GC. Collection
 * is idempotent at the program level — the next trigger picks up
 * whatever the nested allocation produced.
 */
static int corvid_gc_running = 0;

void corvid_gc(void) {
    if (corvid_gc_running) return;
    corvid_gc_running = 1;

    corvid_gc_record_trigger();

    void* base_rbp = corvid_gc_capture_rbp();
    corvid_gc_mark_stack(base_rbp);

    /* Phase 17f++ verifier runs BEFORE sweep so it sees the
     * mark-bit-set reachable set. Passes no explicit roots — the
     * verifier walks from mark-bit-set blocks. */
    if (corvid_gc_verify_mode != 0) {
        corvid_gc_verify(NULL, 0);
    }

    corvid_gc_sweep();

    corvid_gc_running = 0;
}

/* Phase 17d — deterministic variant for tests and controlled
 * scenarios: mark ONLY from the explicit root pointers provided,
 * then sweep. No stack walk. Useful because Rust test binaries
 * don't preserve frame pointers reliably, so the normal
 * `corvid_gc` can behave non-deterministically across test runs
 * depending on exactly where Rust's optimizer placed locals.
 *
 * Also useful in future: 17b-7 latency-aware RC could drive the
 * collector at LLM-call-boundary safepoints where only a few
 * well-known Corvid values are live (the agent's arguments and
 * accumulator), and walking the whole stack would miss the
 * point.
 *
 * `roots` is an array of Corvid-value payload pointers (the same
 * kind of pointer `corvid_alloc_typed` returns). `n_roots` is the
 * array length. Any NULL entries are skipped.
 */
void corvid_gc_from_roots(void** roots, size_t n_roots) {
    if (corvid_gc_running) return;
    corvid_gc_running = 1;

    corvid_gc_record_trigger();

    for (size_t i = 0; i < n_roots; i++) {
        if (roots[i] != NULL) {
            corvid_gc_mark_marker(roots[i], NULL);
        }
    }

    /* Phase 17f++ verifier — uses the now-marked set plus the given
     * explicit roots as edge sources. */
    if (corvid_gc_verify_mode != 0) {
        corvid_gc_verify(roots, n_roots);
    }

    corvid_gc_sweep();

    corvid_gc_running = 0;
}
