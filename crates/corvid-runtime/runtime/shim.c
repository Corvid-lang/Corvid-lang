/* Corvid AOT runtime shim.
 *
 * `main` is no longer here — the codegen emits its own
 * `main(int argc, char** argv)` per program (signature-aware, with
 * argv decoding + result printing tailored to the entry agent's
 * type). This file shrinks to:
 *
 *   - `corvid_runtime_overflow`: the no-return function Cranelift
 *     jumps to on integer overflow, division by zero, or list bounds
 *     violation. Prints to stderr and exits 1.
 *
 * Leak-counter printing moved to `runtime/entry.c`'s `corvid_on_exit`,
 * registered by the generated main via `corvid_init` → `atexit(...)`.
 */

#include <stdio.h>
#include <stdlib.h>
#if defined(_MSC_VER)
#include <windows.h>
#else
#include <time.h>
#endif

void corvid_runtime_overflow(void) {
    fprintf(stderr, "corvid: runtime error: integer overflow, division by zero, or index out of bounds\n");
    exit(1);
}

void corvid_sleep_ms(long long ms) {
    if (ms <= 0) {
        return;
    }
#if defined(_MSC_VER)
    Sleep((DWORD)ms);
#else
    struct timespec ts;
    ts.tv_sec = (time_t)(ms / 1000);
    ts.tv_nsec = (long)((ms % 1000) * 1000000LL);
    nanosleep(&ts, NULL);
#endif
}
