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
#include <stdio.h>

#define CORVID_HEADER_BYTES 16
#define CORVID_REFCOUNT_IMMORTAL ((long long)INT64_MIN)

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

static long long corvid_utf8_char_width(unsigned char byte) {
    if ((byte & 0x80u) == 0) return 1;
    if ((byte & 0xE0u) == 0xC0u) return 2;
    if ((byte & 0xF0u) == 0xE0u) return 3;
    if ((byte & 0xF8u) == 0xF0u) return 4;
    return 1;
}

long long corvid_string_char_len(void* s_payload) {
    corvid_string* s = (corvid_string*)s_payload;
    const unsigned char* bytes = (const unsigned char*)s->bytes_ptr;
    long long remaining = s->length;
    long long count = 0;
    while (remaining > 0) {
        long long width = corvid_utf8_char_width(*bytes);
        if (width > remaining) {
            width = remaining;
        }
        bytes += width;
        remaining -= width;
        count += 1;
    }
    return count;
}

void* corvid_string_char_at(void* s_payload, long long index) {
    corvid_string* s = (corvid_string*)s_payload;
    const unsigned char* bytes = (const unsigned char*)s->bytes_ptr;
    long long remaining = s->length;
    long long current = 0;
    while (remaining > 0) {
        long long width = corvid_utf8_char_width(*bytes);
        if (width > remaining) {
            width = remaining;
        }
        if (current == index) {
            return alloc_string((const char*)bytes, width);
        }
        bytes += width;
        remaining -= width;
        current += 1;
    }
    return alloc_string("", 0);
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

/* Wrap an explicit-length byte slice as an immortal Corvid String.
 * Used by benchmark/runtime fixture paths that repeatedly return the
 * same canned reply and do not benefit from per-use heap allocation.
 *
 * Layout matches a normal String payload:
 *   [ corvid_header ][ corvid_string descriptor ][ bytes ]
 * but the header refcount is the immortal sentinel, so retain/release
 * short-circuit and the descriptor can be reused indefinitely.
 *
 * The allocation is intentionally leaked for process lifetime.
 */
void* corvid_string_from_static_bytes(const char* bytes, long long length) {
    if (length < 0) length = 0;
    size_t total = CORVID_HEADER_BYTES + sizeof(corvid_string) + (size_t)length;
    char* raw = (char*)malloc(total);
    if (raw == NULL) {
        fprintf(stderr, "corvid: out of memory allocating immortal string (%lld bytes)\n", length);
        abort();
    }
    long long* refcount_word = (long long*)raw;
    *refcount_word = CORVID_REFCOUNT_IMMORTAL;
    const struct corvid_typeinfo** typeinfo_ptr =
        (const struct corvid_typeinfo**)(raw + sizeof(long long));
    *typeinfo_ptr = &corvid_typeinfo_String;

    corvid_string* desc = (corvid_string*)(raw + CORVID_HEADER_BYTES);
    char* out_bytes = (char*)desc + sizeof(corvid_string);
    if (length > 0 && bytes != NULL) {
        memcpy(out_bytes, bytes, (size_t)length);
    }
    desc->bytes_ptr = out_bytes;
    desc->length = length;
    return desc;
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

static char corvid_json_hex_digit(unsigned char value) {
    return (char)(value < 10 ? ('0' + value) : ('a' + (value - 10)));
}

void* corvid_string_json_quote(void* s_payload) {
    uint64_t start_ns = corvid_profile_runtime_enabled() ? corvid_now_ns() : 0;
    corvid_string* s = (corvid_string*)s_payload;
    const unsigned char* bytes = (const unsigned char*)s->bytes_ptr;
    long long escaped_len = 2;
    for (long long i = 0; i < s->length; i++) {
        unsigned char ch = bytes[i];
        switch (ch) {
            case '"':
            case '\\':
            case '\b':
            case '\f':
            case '\n':
            case '\r':
            case '\t':
                escaped_len += 2;
                break;
            default:
                escaped_len += (ch < 0x20) ? 6 : 1;
                break;
        }
    }

    void* payload = corvid_alloc_typed(sizeof(corvid_string) + escaped_len,
                                       &corvid_typeinfo_String);
    corvid_string* desc = (corvid_string*)payload;
    char* out = (char*)payload + sizeof(corvid_string);
    long long cursor = 0;
    out[cursor++] = '"';
    for (long long i = 0; i < s->length; i++) {
        unsigned char ch = bytes[i];
        switch (ch) {
            case '"':
                out[cursor++] = '\\';
                out[cursor++] = '"';
                break;
            case '\\':
                out[cursor++] = '\\';
                out[cursor++] = '\\';
                break;
            case '\b':
                out[cursor++] = '\\';
                out[cursor++] = 'b';
                break;
            case '\f':
                out[cursor++] = '\\';
                out[cursor++] = 'f';
                break;
            case '\n':
                out[cursor++] = '\\';
                out[cursor++] = 'n';
                break;
            case '\r':
                out[cursor++] = '\\';
                out[cursor++] = 'r';
                break;
            case '\t':
                out[cursor++] = '\\';
                out[cursor++] = 't';
                break;
            default:
                if (ch < 0x20) {
                    out[cursor++] = '\\';
                    out[cursor++] = 'u';
                    out[cursor++] = '0';
                    out[cursor++] = '0';
                    out[cursor++] = corvid_json_hex_digit((unsigned char)(ch >> 4));
                    out[cursor++] = corvid_json_hex_digit((unsigned char)(ch & 0x0f));
                } else {
                    out[cursor++] = (char)ch;
                }
                break;
        }
    }
    out[cursor++] = '"';
    desc->bytes_ptr = out;
    desc->length = escaped_len;
    if (start_ns != 0) {
        corvid_bench_prompt_render_ns_total += corvid_now_ns() - start_ns;
    }
    return payload;
}
