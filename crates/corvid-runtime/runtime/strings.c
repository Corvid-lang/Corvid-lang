/* Corvid native runtime: String operations.
 *
 * A Corvid String value is a pointer to a 16-byte descriptor that lives
 * immediately after a corvid_header (see alloc.c). Layout:
 *
 *     [ corvid_header (16) ]
 *     [ bytes_ptr (8) | length (8) ]   <-- string value points here
 *     [ bytes (length) ]              <-- only present for heap strings
 *
 * For heap strings (concat results), the descriptor + bytes live in one
 * allocation and bytes_ptr points at the bytes immediately following
 * the descriptor.
 *
 * For static literals, the descriptor lives in `.rodata` with refcount
 * = CORVID_REFCOUNT_IMMORTAL. The bytes_ptr field is a relocation that
 * points at a separately-emitted bytes symbol. retain/release on these
 * are no-ops, so the read-only descriptor is never written to.
 *
 * UTF-8 throughout. Comparison is bytewise (memcmp), which on valid
 * UTF-8 matches Unicode codepoint order for the BMP.
 */

#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#define CORVID_HEADER_BYTES 16

static uint64_t corvid_bench_prompt_render_ns_total = 0;

static uint64_t corvid_now_ns(void) {
    struct timespec ts;
    timespec_get(&ts, TIME_UTC);
    return ((uint64_t)ts.tv_sec * 1000000000ULL) + (uint64_t)ts.tv_nsec;
}

static int corvid_profile_runtime_enabled(void) {
    static int cached = -1;
    if (cached < 0) {
        cached = getenv("CORVID_PROFILE_RUNTIME") != NULL ? 1 : 0;
    }
    return cached;
}

uint64_t corvid_bench_prompt_render_ns(void) {
    return corvid_bench_prompt_render_ns_total;
}

/* Typeinfo block forward-declaration — the definition lives in alloc.c
 * (`corvid_typeinfo_String`). Every runtime-internal string allocation
 * tags its header with this block so the collector tracer can dispatch. */
struct corvid_typeinfo;
extern const struct corvid_typeinfo corvid_typeinfo_String;
extern void* corvid_alloc_typed(long long payload_bytes,
                                const struct corvid_typeinfo* typeinfo);

typedef struct {
    const char* bytes_ptr;
    long long length;
} corvid_string;

/* Allocate a fresh String with `len` bytes, copying from `src`. The
 * descriptor and bytes share one allocation; bytes_ptr points at the
 * memory immediately following the descriptor (same allocation block).
 * Returns the descriptor pointer (refcount = 1).
 */
static void* alloc_string(const char* src, long long len) {
    /* descriptor (16 bytes) + bytes (len) in one alloc block */
    void* payload = corvid_alloc_typed(sizeof(corvid_string) + len,
                                       &corvid_typeinfo_String);
    corvid_string* desc = (corvid_string*)payload;
    char* bytes = (char*)payload + sizeof(corvid_string);
    if (len > 0 && src != NULL) {
        memcpy(bytes, src, (size_t)len);
    }
    desc->bytes_ptr = bytes;
    desc->length = len;
    return payload;
}

/* Concatenate two strings. Returns a freshly-allocated String at
 * refcount = 1. Does not modify or retain the inputs.
 */
void* corvid_string_concat(void* a_payload, void* b_payload) {
    uint64_t start_ns = corvid_profile_runtime_enabled() ? corvid_now_ns() : 0;
    corvid_string* a = (corvid_string*)a_payload;
    corvid_string* b = (corvid_string*)b_payload;
    long long total = a->length + b->length;
    void* payload = corvid_alloc_typed(sizeof(corvid_string) + total,
                                       &corvid_typeinfo_String);
    corvid_string* desc = (corvid_string*)payload;
    char* bytes = (char*)payload + sizeof(corvid_string);
    if (a->length > 0) memcpy(bytes, a->bytes_ptr, (size_t)a->length);
    if (b->length > 0) memcpy(bytes + a->length, b->bytes_ptr, (size_t)b->length);
    desc->bytes_ptr = bytes;
    desc->length = total;
    if (start_ns != 0) {
        corvid_bench_prompt_render_ns_total += corvid_now_ns() - start_ns;
    }
    return payload;
}

/* Read-only equality. Returns 1 if equal byte-for-byte, 0 otherwise. */
long long corvid_string_eq(void* a_payload, void* b_payload) {
    corvid_string* a = (corvid_string*)a_payload;
    corvid_string* b = (corvid_string*)b_payload;
    if (a->length != b->length) return 0;
    if (a->length == 0) return 1;
    return memcmp(a->bytes_ptr, b->bytes_ptr, (size_t)a->length) == 0 ? 1 : 0;
}

/* Read-only ordering. Returns -1 if a < b, 0 if a == b, 1 if a > b.
 * Bytewise (memcmp) — matches the interpreter's `String < String`
 * lowering which uses Rust's slice ordering.
 */
long long corvid_string_cmp(void* a_payload, void* b_payload) {
    corvid_string* a = (corvid_string*)a_payload;
    corvid_string* b = (corvid_string*)b_payload;
    long long min_len = a->length < b->length ? a->length : b->length;
    int rc = 0;
    if (min_len > 0) rc = memcmp(a->bytes_ptr, b->bytes_ptr, (size_t)min_len);
    if (rc != 0) return rc < 0 ? -1 : 1;
    if (a->length < b->length) return -1;
    if (a->length > b->length) return 1;
    return 0;
}

/* Wrap a null-terminated C string as a refcounted Corvid String.
 * Used by the codegen-emitted main to convert argv[i] arguments into
 * String values for entry agents that take String parameters.
 * Returns at refcount = 1 — callee takes ownership.
 */
void* corvid_string_from_cstr(const char* s) {
    if (s == NULL) {
        return alloc_string("", 0);
    }
    long long len = (long long)strlen(s);
    return alloc_string(s, len);
}

/* Wrap an explicit-length byte slice as a refcounted Corvid String.
 * Consumed by the Rust-side `string_from_rust` helper in
 * `corvid-runtime::ffi_bridge` when a `#[tool]` wrapper converts a
 * returned Rust `String` back into a `CorvidString`. NUL bytes within
 * the payload are preserved — this is not strcpy.
 *
 * Returns at refcount = 1; caller takes ownership.
 */
void* corvid_string_from_bytes(const char* bytes, long long length) {
    if (length < 0) length = 0;
    return alloc_string(bytes, length);
}

/* Scalar-to-String stringification helpers used by the
 * Cranelift codegen when interpolating prompt-template `{var}`
 * placeholders whose argument is a non-String scalar. The rendered
 * prompt is built up by concatenating template literals and these
 * stringified scalars before the LLM call.
 *
 * Each returns a fresh refcounted Corvid String at refcount = 1;
 * caller takes ownership.
 */

#include <stdio.h>

void* corvid_string_from_int(long long n) {
    uint64_t start_ns = corvid_profile_runtime_enabled() ? corvid_now_ns() : 0;
    /* `%lld` with a 32-byte buffer covers every possible i64 (max
     * decimal length of i64 is 20 chars including the sign). */
    char buf[32];
    int len = snprintf(buf, sizeof(buf), "%lld", n);
    if (len < 0) len = 0;
    void* payload = alloc_string(buf, (long long)len);
    if (start_ns != 0) {
        corvid_bench_prompt_render_ns_total += corvid_now_ns() - start_ns;
    }
    return payload;
}

void* corvid_string_from_bool(char b) {
    uint64_t start_ns = corvid_profile_runtime_enabled() ? corvid_now_ns() : 0;
    /* Match the user-visible Bool format Corvid uses on stdout
     * (`corvid_print_bool`): `true` / `false`, lowercase.
     * Templates embedding Bool values get the same string literal
     * a user would see when printing. */
    if (b) {
        void* payload = alloc_string("true", 4);
        if (start_ns != 0) {
            corvid_bench_prompt_render_ns_total += corvid_now_ns() - start_ns;
        }
        return payload;
    } else {
        void* payload = alloc_string("false", 5);
        if (start_ns != 0) {
            corvid_bench_prompt_render_ns_total += corvid_now_ns() - start_ns;
        }
        return payload;
    }
}

void* corvid_string_from_float(double v) {
    uint64_t start_ns = corvid_profile_runtime_enabled() ? corvid_now_ns() : 0;
    /* `%.17g` is the round-trippable IEEE 754 format — same one
     * `corvid_print_f64` uses for stdout. NaN / Inf round-trip
     * as their printf representations ("nan" / "inf" / "-inf"). 64
     * bytes is conservatively above the longest possible %.17g
     * output (~25 chars). */
    char buf[64];
    int len = snprintf(buf, sizeof(buf), "%.17g", v);
    if (len < 0) len = 0;
    void* payload = alloc_string(buf, (long long)len);
    if (start_ns != 0) {
        corvid_bench_prompt_render_ns_total += corvid_now_ns() - start_ns;
    }
    return payload;
}
