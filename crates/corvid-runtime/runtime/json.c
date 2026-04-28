/* Corvid native runtime: JSON stringification for trace payloads.
 *
 * The native trace recorder slots non-scalar tool/prompt/approve args
 * into the trace as JSON. Codegen-cl walks the type structurally and
 * drives this buffer to emit JSON bytes for one argument; the caller
 * finalizes the buffer into a refcounted Corvid String and stores the
 * descriptor pointer in the trace slot. The runtime decoder then reads
 * the descriptor as JSON via serde_json (see `native_trace.rs`).
 *
 * Lifetime: a buffer is allocated by `corvid_json_buffer_new`, written
 * to by the append primitives during one trace-payload emission, and
 * destroyed by `corvid_json_buffer_finish` which returns a fresh
 * refcounted Corvid String containing the accumulated JSON.
 *
 * Encoding follows RFC 8259: control chars escaped, `\` and `"`
 * escaped, valid UTF-8 passes through bytewise. Non-finite doubles
 * become `null` because the JSON spec has no NaN/Infinity tokens —
 * matches the existing `serde_json::Number::from_f64` fallback in the
 * trace decoder.
 */

#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include <math.h>

struct corvid_typeinfo;
extern const struct corvid_typeinfo corvid_typeinfo_String;
extern void* corvid_alloc_typed(long long payload_bytes,
                                const struct corvid_typeinfo* typeinfo);

typedef struct {
    const char* bytes_ptr;
    long long length;
} corvid_string;

typedef struct {
    char* bytes;
    size_t length;
    size_t capacity;
} corvid_json_buffer;

static void buffer_grow(corvid_json_buffer* buf, size_t additional) {
    size_t needed = buf->length + additional;
    if (needed <= buf->capacity) return;
    size_t new_cap = buf->capacity == 0 ? 64 : buf->capacity * 2;
    while (new_cap < needed) new_cap *= 2;
    char* fresh = (char*)realloc(buf->bytes, new_cap);
    if (fresh == NULL) {
        fprintf(stderr, "corvid: corvid_json_buffer realloc failed\n");
        abort();
    }
    buf->bytes = fresh;
    buf->capacity = new_cap;
}

static void buffer_append_bytes(corvid_json_buffer* buf, const char* src, size_t len) {
    if (len == 0) return;
    buffer_grow(buf, len);
    memcpy(buf->bytes + buf->length, src, len);
    buf->length += len;
}

static void buffer_append_byte(corvid_json_buffer* buf, char ch) {
    buffer_grow(buf, 1);
    buf->bytes[buf->length++] = ch;
}

void* corvid_json_buffer_new(void) {
    corvid_json_buffer* buf = (corvid_json_buffer*)malloc(sizeof(corvid_json_buffer));
    if (buf == NULL) {
        fprintf(stderr, "corvid: corvid_json_buffer alloc failed\n");
        abort();
    }
    buf->bytes = NULL;
    buf->length = 0;
    buf->capacity = 0;
    return buf;
}

/* Append literal JSON syntax bytes from a Corvid String descriptor.
 * Used for delimiters and pre-quoted field names emitted by codegen
 * as `.rodata` String literals. The bytes are treated as already-valid
 * JSON text and copied verbatim — no escaping applied.
 */
void corvid_json_buffer_append_raw(void* buf_ptr, void* desc_ptr) {
    corvid_json_buffer* buf = (corvid_json_buffer*)buf_ptr;
    corvid_string* desc = (corvid_string*)desc_ptr;
    if (desc->length > 0) {
        buffer_append_bytes(buf, desc->bytes_ptr, (size_t)desc->length);
    }
}

void corvid_json_buffer_append_int(void* buf_ptr, long long value) {
    char tmp[32];
    int written = snprintf(tmp, sizeof(tmp), "%lld", value);
    if (written > 0) {
        buffer_append_bytes((corvid_json_buffer*)buf_ptr, tmp, (size_t)written);
    }
}

void corvid_json_buffer_append_float(void* buf_ptr, double value) {
    corvid_json_buffer* buf = (corvid_json_buffer*)buf_ptr;
    if (!isfinite(value)) {
        /* JSON has no representation for NaN/Inf; mirror the
         * `serde_json::Number::from_f64` fallback path which writes
         * `null` rather than emitting invalid JSON.
         */
        buffer_append_bytes(buf, "null", 4);
        return;
    }
    char tmp[64];
    /* `%.17g` is the canonical IEEE-754 round-trip format for f64. */
    int written = snprintf(tmp, sizeof(tmp), "%.17g", value);
    if (written <= 0) {
        buffer_append_bytes(buf, "null", 4);
        return;
    }
    buffer_append_bytes(buf, tmp, (size_t)written);
}

void corvid_json_buffer_append_bool(void* buf_ptr, signed char value) {
    if (value != 0) {
        buffer_append_bytes((corvid_json_buffer*)buf_ptr, "true", 4);
    } else {
        buffer_append_bytes((corvid_json_buffer*)buf_ptr, "false", 5);
    }
}

void corvid_json_buffer_append_null(void* buf_ptr) {
    buffer_append_bytes((corvid_json_buffer*)buf_ptr, "null", 4);
}

/* Append a Corvid String value as a JSON string: surrounding quotes plus
 * RFC 8259 escaping of `"`, `\`, and 0x00–0x1F control characters.
 * Multi-byte UTF-8 sequences pass through unchanged.
 */
void corvid_json_buffer_append_string(void* buf_ptr, void* desc_ptr) {
    corvid_json_buffer* buf = (corvid_json_buffer*)buf_ptr;
    corvid_string* desc = (corvid_string*)desc_ptr;
    buffer_append_byte(buf, '"');
    const char* src = desc->bytes_ptr;
    long long remaining = desc->length;
    long long idx = 0;
    while (idx < remaining) {
        unsigned char ch = (unsigned char)src[idx];
        if (ch == '"' || ch == '\\') {
            buffer_append_byte(buf, '\\');
            buffer_append_byte(buf, (char)ch);
        } else if (ch == '\b') {
            buffer_append_bytes(buf, "\\b", 2);
        } else if (ch == '\t') {
            buffer_append_bytes(buf, "\\t", 2);
        } else if (ch == '\n') {
            buffer_append_bytes(buf, "\\n", 2);
        } else if (ch == '\f') {
            buffer_append_bytes(buf, "\\f", 2);
        } else if (ch == '\r') {
            buffer_append_bytes(buf, "\\r", 2);
        } else if (ch < 0x20) {
            char esc[8];
            int written = snprintf(esc, sizeof(esc), "\\u%04x", ch);
            if (written > 0) {
                buffer_append_bytes(buf, esc, (size_t)written);
            }
        } else {
            buffer_append_byte(buf, (char)ch);
        }
        idx++;
    }
    buffer_append_byte(buf, '"');
}

/* Finalize the buffer into a fresh refcounted Corvid String (refcount=1)
 * containing the accumulated JSON bytes. The buffer itself is freed.
 * Returns the descriptor pointer; callers must release it like any
 * other Corvid String.
 */
void* corvid_json_buffer_finish(void* buf_ptr) {
    corvid_json_buffer* buf = (corvid_json_buffer*)buf_ptr;
    long long len = (long long)buf->length;
    void* payload = corvid_alloc_typed(sizeof(corvid_string) + len,
                                       &corvid_typeinfo_String);
    corvid_string* desc = (corvid_string*)payload;
    char* bytes = (char*)payload + sizeof(corvid_string);
    if (len > 0) memcpy(bytes, buf->bytes, (size_t)len);
    desc->bytes_ptr = bytes;
    desc->length = len;
    free(buf->bytes);
    free(buf);
    return payload;
}
