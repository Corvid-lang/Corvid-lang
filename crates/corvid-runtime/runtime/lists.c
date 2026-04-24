/* Corvid native runtime: List destroy + trace (typeinfo-driven).
 *
 * Layout of a Corvid list (single allocation):
 *
 *   [ corvid_header (16) ]
 *   [ length (8) | element_0 (8) | element_1 (8) | ... ]
 *                 ^-- list value points here (descriptor)
 *
 * The descriptor's offset 0 = length; offsets 8, 16, 24, ... = elements.
 * Each element is a single I64 (either a scalar value or a refcounted
 * pointer, depending on the element type).
 *
 * Typed headers: one destroy and one trace function serve
 * every refcounted-element list type. Both recover the list's typeinfo
 * from the header (walking back 16 bytes), read `elem_typeinfo` from
 * it, and decide whether to follow elements:
 *
 *   - elem_typeinfo == NULL  → primitive elements (List<Int>, etc.).
 *     destroy_fn is NEVER installed for these (codegen leaves it NULL
 *     so `corvid_release` skips destruction). trace_fn is installed
 *     but is a no-op walk.
 *   - elem_typeinfo != NULL  → refcounted elements (List<String>,
 *     List<Struct>, List<List<T>>). destroy releases each; trace
 *     invokes the marker on each.
 *
 * This replaces the pre-typed-header shared destructor, which relied
 * on the header's single `reserved` slot as a destructor pointer and
 * therefore could not distinguish primitive-element lists at the
 * trace level — only at the destroy level.
 */

#include <stddef.h>
#include <stdint.h>

#define CORVID_HEADER_BYTES 16

struct corvid_typeinfo {
    uint32_t size;
    uint32_t flags;
    void (*destroy_fn)(void*);
    void (*trace_fn)(void*, void (*marker)(void*, void*), void*);
    void (*weak_fn)(void*);
    const struct corvid_typeinfo* elem_typeinfo;
    const char* name;
};

typedef struct {
    long long refcount_word;
    const struct corvid_typeinfo* typeinfo;
} corvid_header;

extern void corvid_release(void* payload);

/* Shared destroy — installed on every refcounted-element list's typeinfo.
 * Walks the length prefix and releases each element. corvid_release
 * then frees the list's own block after this returns.
 */
void corvid_destroy_list(void* payload) {
    long long* p = (long long*)payload;
    long long length = p[0];
    for (long long i = 0; i < length; i++) {
        void* elem = (void*)(intptr_t)p[1 + i];
        if (elem != NULL) {
            corvid_release(elem);
        }
    }
}

/* Shared trace — installed on every list's typeinfo (refcounted-element
 * AND primitive-element). Invokes `marker(elem, ctx)` for each
 * refcounted element. No-op for primitive-element lists (elem_typeinfo
 * NULL) — prevents the List<Int> mis-trace bug the pre-typed-header
 * design would have hit (tracing Int slots as pointers).
 */
void corvid_trace_list(void* payload,
                       void (*marker)(void*, void*),
                       void* ctx) {
    corvid_header* h = (corvid_header*)((char*)payload - CORVID_HEADER_BYTES);
    if (h->typeinfo == NULL || h->typeinfo->elem_typeinfo == NULL) {
        /* primitive elements — nothing to trace */
        return;
    }
    long long* p = (long long*)payload;
    long long length = p[0];
    for (long long i = 0; i < length; i++) {
        void* elem = (void*)(intptr_t)p[1 + i];
        if (elem != NULL) {
            marker(elem, ctx);
        }
    }
}
