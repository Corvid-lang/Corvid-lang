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

#define CORVID_HEADER_BYTES 16

extern void* corvid_alloc(long long payload_bytes);

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
    void* payload = corvid_alloc(sizeof(corvid_string) + len);
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
    corvid_string* a = (corvid_string*)a_payload;
    corvid_string* b = (corvid_string*)b_payload;
    long long total = a->length + b->length;
    void* payload = corvid_alloc(sizeof(corvid_string) + total);
    corvid_string* desc = (corvid_string*)payload;
    char* bytes = (char*)payload + sizeof(corvid_string);
    if (a->length > 0) memcpy(bytes, a->bytes_ptr, (size_t)a->length);
    if (b->length > 0) memcpy(bytes + a->length, b->bytes_ptr, (size_t)b->length);
    desc->bytes_ptr = bytes;
    desc->length = total;
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
