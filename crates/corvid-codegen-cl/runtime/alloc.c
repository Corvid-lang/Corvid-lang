/* Corvid native runtime: refcounted heap allocations.
 *
 * Every refcounted Corvid value (String, future Struct, future List)
 * sits behind a 16-byte header:
 *
 *     [ atomic_refcount (8) | reserved (8) ]
 *     [ payload bytes...                   ]  <-- corvid_alloc returns here
 *
 * The header lives immediately before the payload pointer the runtime
 * exposes. `corvid_retain` / `corvid_release` walk back 16 bytes to
 * find it.
 *
 * Atomic refcount: Corvid is single-threaded today, but Phase 25
 * multi-agent will introduce real concurrency. Going atomic now means
 * we don't have to migrate every binary in the wild later. Cost is
 * ~10-50ns per ref change vs ~1-2ns non-atomic — small and worth it.
 *
 * Immortal sentinel: static literals (descriptors emitted in `.rodata`)
 * have refcount = INT64_MIN. retain/release short-circuit on this
 * value, so reads of literal strings never touch the static memory's
 * (read-only) refcount field.
 *
 * Leak detector: two atomic counters track total allocations and total
 * releases. The shim prints them on exit when CORVID_DEBUG_ALLOC is
 * set. Parity tests assert the two are equal — a missed release means
 * the codegen forgot a `corvid_release` somewhere.
 */

#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <inttypes.h>

#define CORVID_HEADER_BYTES 16

/* Sentinel marking a "never collect" allocation — used by static literals. */
#define CORVID_REFCOUNT_IMMORTAL ((long long)INT64_MIN)

typedef struct {
    _Atomic long long refcount;
    long long reserved;          /* future: type tag, weak count, etc. */
} corvid_header;

/* ---- leak detector counters ---------------------------------------- */

_Atomic long long corvid_alloc_count = 0;
_Atomic long long corvid_release_count = 0;

/* ---- runtime API exposed to compiled code -------------------------- */

/* Allocate `payload_bytes` bytes of payload behind a 16-byte header.
 * Returns a pointer to the payload (not the header). Initial refcount
 * is 1 — one owner. No destructor.
 */
void* corvid_alloc(long long payload_bytes) {
    if (payload_bytes < 0) {
        fprintf(stderr, "corvid: corvid_alloc called with negative size %lld\n",
                payload_bytes);
        exit(1);
    }
    char* block = (char*)malloc(CORVID_HEADER_BYTES + (size_t)payload_bytes);
    if (block == NULL) {
        fprintf(stderr, "corvid: out of memory (requested %lld bytes)\n",
                payload_bytes);
        exit(1);
    }
    corvid_header* h = (corvid_header*)block;
    atomic_store_explicit(&h->refcount, (long long)1, memory_order_relaxed);
    h->reserved = 0;
    atomic_fetch_add_explicit(&corvid_alloc_count, 1, memory_order_relaxed);
    return (void*)(block + CORVID_HEADER_BYTES);
}

/* Like corvid_alloc but stores a destructor function pointer in the
 * header's reserved slot. `corvid_release` calls `destructor(payload)`
 * just before freeing the block when refcount hits 0 — used by Struct
 * (and future List / nested-Struct) allocations to release refcounted
 * fields before the container itself is freed.
 *
 * Destructor signature: `void (*)(void* payload)`. The destructor
 * receives the payload pointer (same as what corvid_release got) and
 * is responsible for releasing any refcounted members. It must NOT
 * free the block itself — corvid_release handles that.
 */
void* corvid_alloc_with_destructor(long long payload_bytes,
                                   void (*destructor)(void*)) {
    void* payload = corvid_alloc(payload_bytes);
    corvid_header* h = (corvid_header*)((char*)payload - CORVID_HEADER_BYTES);
    h->reserved = (long long)(intptr_t)destructor;
    return payload;
}

/* Increment the refcount of `payload`. No-op for immortal allocations. */
void corvid_retain(void* payload) {
    if (payload == NULL) return;
    corvid_header* h = (corvid_header*)((char*)payload - CORVID_HEADER_BYTES);
    long long current = atomic_load_explicit(&h->refcount, memory_order_relaxed);
    if (current == CORVID_REFCOUNT_IMMORTAL) return;
    atomic_fetch_add_explicit(&h->refcount, 1, memory_order_relaxed);
}

/* Decrement the refcount; free when it hits zero. No-op for immortals.
 *
 * `corvid_release_count` only increments when an allocation actually
 * gets freed (refcount reached zero) — *not* on every release call —
 * so it pairs 1:1 with `corvid_alloc_count` for leak detection. The
 * parity harness asserts the two are equal at process exit; any
 * imbalance means the codegen forgot a release somewhere.
 */
void corvid_release(void* payload) {
    if (payload == NULL) return;
    corvid_header* h = (corvid_header*)((char*)payload - CORVID_HEADER_BYTES);
    long long current = atomic_load_explicit(&h->refcount, memory_order_relaxed);
    if (current == CORVID_REFCOUNT_IMMORTAL) return;
    long long previous = atomic_fetch_sub_explicit(&h->refcount, 1, memory_order_acq_rel);
    if (previous == 1) {
        /* Run the type's destructor (if any) before freeing — releases
         * refcounted fields of Structs, elements of Lists, etc. */
        if (h->reserved != 0) {
            typedef void (*corvid_destructor)(void*);
            ((corvid_destructor)(intptr_t)h->reserved)(payload);
        }
        atomic_fetch_add_explicit(&corvid_release_count, 1, memory_order_relaxed);
        free((void*)h);
    } else if (previous <= 0) {
        fprintf(stderr,
                "corvid: corvid_release on already-freed allocation (refcount was %lld)\n",
                previous);
        exit(1);
    }
    /* `previous > 1`: the allocation has other owners still holding
     * references. No leak counter increment — leak detection cares
     * about freed allocations, not about every release call. */
}
