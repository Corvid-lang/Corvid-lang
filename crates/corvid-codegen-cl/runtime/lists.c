/* Corvid native runtime: List destructor for refcounted-element lists.
 *
 * Layout of a Corvid list (single allocation):
 *
 *   [ corvid_header (16) ]
 *   [ length (8) | element_0 (8) | element_1 (8) | ... ]
 *                 ^-- list value points here (descriptor)
 *
 * The descriptor's offset 0 = length; offsets 8, 16, 24, ... = elements.
 * Each element is a single I64 (either a scalar value or a refcounted
 * pointer, depending on the element type). This C destructor is shared
 * across every refcounted-element list type — regardless of whether T
 * is String, Struct, or nested List, a refcounted element is always
 * "an I64 pointer that needs release". `corvid_release` handles the
 * per-type cleanup of each element via its own header chain.
 *
 * Non-refcounted-element lists (List<Int>, List<Bool>, List<Float>)
 * keep `reserved = 0` and never invoke a destructor.
 */

#include <stdatomic.h>
#include <stdint.h>

extern void corvid_release(void* payload);

/* Generic destructor: walk length at offset 0, release each element.
 * Matches the alloc layout: list descriptor points at the payload (past
 * the 16-byte header); elements live at offsets 8, 16, 24, ... from
 * the descriptor. `corvid_release` then frees the list's own block
 * after this function returns.
 */
void corvid_destroy_list_refcounted(void* payload) {
    long long* p = (long long*)payload;
    long long length = p[0];
    for (long long i = 0; i < length; i++) {
        /* Each element is a pointer to another payload (String, Struct,
         * nested List). Passing it to corvid_release walks its own
         * header and decrements its refcount; if that hits 0, its
         * own destructor runs (recursively cleaning nested types). */
        void* elem = (void*)(intptr_t)p[1 + i];
        if (elem != NULL) {
            corvid_release(elem);
        }
    }
}
