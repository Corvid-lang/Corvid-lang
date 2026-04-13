/* Corvid AOT entry shim + runtime overflow handler.
 *
 * Linked with every compiled Corvid binary. The Corvid compiler emits
 * each agent as a Cranelift-defined external symbol. This shim:
 *
 *   1. Defines `main`, which calls the entry agent (always named
 *      `corvid_entry` — the driver renames or aliases the user's entry
 *      agent to that symbol at link time).
 *   2. Prints the returned i64 to stdout with a trailing newline.
 *   3. Defines `corvid_runtime_overflow` — the no-return function
 *      Cranelift jumps to when an arithmetic overflow or division-by-zero
 *      is detected. Prints a parity-matching error to stderr and exits 1.
 *
 * Slice 12a: Int-only, parameter-less entry. Parameterised entry plus
 * command-line argv handling arrives in later slices as the type
 * surface widens.
 */

#include <stdio.h>
#include <stdlib.h>

extern long long corvid_entry(void);

void corvid_runtime_overflow(void) {
    fprintf(stderr, "corvid: runtime error: integer overflow or division by zero\n");
    exit(1);
}

int main(void) {
    long long result = corvid_entry();
    printf("%lld\n", result);
    return 0;
}
