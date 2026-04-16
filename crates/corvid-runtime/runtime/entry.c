/* Corvid native runtime: entry-agent helpers.
 *
 * The codegen emits its own `main(int argc, char** argv)`
 * via Cranelift, signature-aware per program. That generated main
 * uses the helpers in this file to:
 *
 *   - decode argv[i+1] into typed Corvid values (parse_i64 / _f64 /
 *     _bool / string_from_cstr — the last lives in strings.c since
 *     it allocates a String descriptor)
 *   - print the entry agent's return value (print_i64 / _f64 / _string)
 *   - report arity mismatches with a clear message
 *   - register the leak-detector printer via atexit (corvid_init →
 *     atexit(corvid_on_exit))
 *
 * Parse / arity errors print a clear message and `exit(1)`.
 * The runtime overflow handler stays for arithmetic and bounds
 * violations; entry-time parse errors get their own messages because
 * they're the user's first interaction with the binary and the
 * generic "integer overflow" wording would mislead.
 */

#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <stdlib.h>

extern long long corvid_alloc_count;
extern long long corvid_release_count;
extern long long corvid_retain_call_count;
extern long long corvid_release_call_count;
/* Set by corvid_init based on CORVID_GC_TRIGGER env var.
 * alloc.c reads this to decide when to auto-collect. */
extern long long corvid_gc_trigger_threshold;

/* Verifier mode, set by corvid_init based on
 * CORVID_GC_VERIFY env var. 0=off, 1=warn, 2=abort. verify.c reads
 * this at every GC cycle. */
extern int corvid_gc_verify_mode;
extern long long corvid_gc_verify_drift_count;

/* ---- exit-time leak + RC-op counters (registered via corvid_init) --- */

void corvid_on_exit(void) {
    if (getenv("CORVID_DEBUG_ALLOC")) {
        fprintf(stderr,
                "ALLOCS=%lld\nRELEASES=%lld\nRETAIN_CALLS=%lld\nRELEASE_CALLS=%lld\n",
                corvid_alloc_count,
                corvid_release_count,
                corvid_retain_call_count,
                corvid_release_call_count);
    }
    /* If the verifier ran and caught drift, surface
     * the total at exit even under warn mode so CI can pick it up. */
    if (corvid_gc_verify_mode != 0 && corvid_gc_verify_drift_count > 0) {
        fprintf(stderr,
                "CORVID_GC_VERIFY: %lld total drift report(s) this run\n",
                corvid_gc_verify_drift_count);
    }
}

extern void corvid_stack_maps_dump(void);
extern int corvid_stack_maps_dump_requested;

/* Called as the first instruction of generated main. Registers the
 * exit handler so leak counters get printed regardless of how main
 * eventually returns. Also dumps the stack-map table
 * when CORVID_DEBUG_STACK_MAPS is set, so the integration test can
 * inspect what codegen emitted.
 */
void corvid_init(void) {
    atexit(corvid_on_exit);
    /* Dump-on-start if requested. The flag is a simple
     * int (not a getenv call here) so stack_maps.o doesn't need
     * getenv, keeping the minimal-CRT test link simple. */
    corvid_stack_maps_dump_requested =
        (getenv("CORVID_DEBUG_STACK_MAPS") != NULL) ? 1 : 0;
    if (corvid_stack_maps_dump_requested) {
        corvid_stack_maps_dump();
    }
    /* Parse CORVID_GC_TRIGGER here rather than in
     * alloc.c; keeps strtoll/getenv out of alloc.o so the minimal-
     * CRT tests (ffi_bridge_smoke) can link corvid_c_runtime without
     * dragging in full stdlib. Default: 10_000 allocations between
     * automatic GC cycles. Set to 0 to disable auto-GC. */
    const char* v = getenv("CORVID_GC_TRIGGER");
    if (v != NULL) {
        char* end = NULL;
        long long n = strtoll(v, &end, 10);
        corvid_gc_trigger_threshold = (n >= 0 && end != v) ? n : 10000;
    } else {
        corvid_gc_trigger_threshold = 10000;
    }

    /* Verifier mode. off|warn|abort. */
    const char* vv = getenv("CORVID_GC_VERIFY");
    if (vv != NULL) {
        if (strcmp(vv, "warn") == 0 || strcmp(vv, "1") == 0) {
            corvid_gc_verify_mode = 1;
        } else if (strcmp(vv, "abort") == 0 || strcmp(vv, "2") == 0) {
            corvid_gc_verify_mode = 2;
        } else {
            corvid_gc_verify_mode = 0;
        }
    }
}

/* ---- arity check -------------------------------------------------- */

void corvid_arity_mismatch(long long expected, long long got) {
    fprintf(stderr,
            "corvid: program expects %lld argument(s), got %lld\n",
            expected, got);
    exit(2);
}

/* ---- parse helpers ------------------------------------------------ */

long long corvid_parse_i64(const char* s, long long argv_index) {
    if (s == NULL) {
        fprintf(stderr,
                "corvid: argv[%lld] is missing — expected Int\n",
                argv_index);
        exit(1);
    }
    char* end = NULL;
    errno = 0;
    long long n = strtoll(s, &end, 10);
    if (errno != 0 || end == s || *end != '\0') {
        fprintf(stderr,
                "corvid: cannot parse argv[%lld] = \"%s\" as Int\n",
                argv_index, s);
        exit(1);
    }
    return n;
}

double corvid_parse_f64(const char* s, long long argv_index) {
    if (s == NULL) {
        fprintf(stderr,
                "corvid: argv[%lld] is missing — expected Float\n",
                argv_index);
        exit(1);
    }
    char* end = NULL;
    errno = 0;
    double v = strtod(s, &end);
    if (errno != 0 || end == s || *end != '\0') {
        fprintf(stderr,
                "corvid: cannot parse argv[%lld] = \"%s\" as Float\n",
                argv_index, s);
        exit(1);
    }
    return v;
}

/* Returns 0 (false) or 1 (true). Accepts case-insensitive
 * "true"/"false" and "1"/"0". Anything else is an error.
 */
char corvid_parse_bool(const char* s, long long argv_index) {
    if (s != NULL) {
        if (strcmp(s, "true") == 0 || strcmp(s, "True") == 0
            || strcmp(s, "TRUE") == 0 || strcmp(s, "1") == 0) {
            return 1;
        }
        if (strcmp(s, "false") == 0 || strcmp(s, "False") == 0
            || strcmp(s, "FALSE") == 0 || strcmp(s, "0") == 0) {
            return 0;
        }
    }
    fprintf(stderr,
            "corvid: cannot parse argv[%lld] = \"%s\" as Bool (expected true / false / 1 / 0)\n",
            argv_index, s ? s : "(null)");
    exit(1);
}

/* ---- print helpers ----------------------------------------------- */

void corvid_print_i64(long long v) {
    printf("%lld\n", v);
}

void corvid_print_bool(long long v) {
    /* Treat any non-zero as true. The codegen passes 0 or 1; this
     * matches what users expect at the command line. */
    printf("%s\n", v ? "true" : "false");
}

void corvid_print_f64(double v) {
    /* %.17g is round-trippable for IEEE 754 doubles. */
    printf("%.17g\n", v);
}

/* Print a Corvid String — descriptor at offset 0 = bytes_ptr,
 * offset 8 = length. */
void corvid_print_string(void* descriptor) {
    if (descriptor == NULL) {
        printf("\n");
        return;
    }
    long long* desc = (long long*)descriptor;
    const char* bytes = (const char*)(intptr_t)desc[0];
    long long length = desc[1];
    if (length > 0 && bytes != NULL) {
        fwrite(bytes, 1, (size_t)length, stdout);
    }
    fputc('\n', stdout);
}
