/* Corvid AOT entry shim + runtime overflow handler.
 *
 * Linked with every compiled Corvid binary. The codegen emits a
 * `corvid_entry` trampoline that calls the chosen entry agent and
 * returns its `i64` result. This shim:
 *
 *   1. Defines `main`, which invokes `corvid_entry`.
 *   2. Prints the returned i64 to stdout with a trailing newline.
 *   3. When `CORVID_DEBUG_ALLOC` is set, prints the runtime allocation
 *      counters to stderr after the agent returns. Parity tests use
 *      this to assert no allocations leak.
 *   4. Defines `corvid_runtime_overflow` — the no-return function
 *      Cranelift jumps to on integer overflow / division by zero.
 *      Prints to stderr and exits 1.
 *
 * Slice 12a-c-d-e: Int / Bool / Float / String. Parameterless entry,
 * Int or Bool return. Parameterised entries + non-Int returns arrive
 * in slice 12h.
 */

#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>

extern long long corvid_entry(void);

extern _Atomic long long corvid_alloc_count;
extern _Atomic long long corvid_release_count;

void corvid_runtime_overflow(void) {
    fprintf(stderr, "corvid: runtime error: integer overflow or division by zero\n");
    exit(1);
}

int main(void) {
    long long result = corvid_entry();
    printf("%lld\n", result);
    if (getenv("CORVID_DEBUG_ALLOC")) {
        long long allocs =
            atomic_load_explicit(&corvid_alloc_count, memory_order_relaxed);
        long long releases =
            atomic_load_explicit(&corvid_release_count, memory_order_relaxed);
        fprintf(stderr, "ALLOCS=%lld\nRELEASES=%lld\n", allocs, releases);
    }
    return 0;
}
