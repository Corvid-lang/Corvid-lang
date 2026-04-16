/* Corvid native runtime: refcounted heap allocations.
 *
 * Every refcounted Corvid value (String, Struct, List) sits behind a
 * 16-byte header:
 *
 *     [ refcount_word (8) | typeinfo_ptr (8) ]
 *     [ payload bytes...                     ]  <-- alloc returns here
 *
 * Header layout:
 *
 *   refcount_word (i64), bit-packed:
 *     bits  0..60  refcount
 *     bit   61     mark bit for the native cycle collector
 *     bit   62     color bit used by the VM Bacon-Rajan collector
 *     bit   63     sign bit reserved for the INT64_MIN immortal sentinel
 *
 *   typeinfo_ptr (const corvid_typeinfo*):
 *     points at a per-type metadata block emitted in .rodata by the
 *     codegen (or the runtime for built-ins like String).
 *
 * Hidden tracking-node prefix for cycle collection:
 *
 * Heap allocations actually allocate 24 bytes of hidden prefix BEFORE
 * the user-visible 16-byte header:
 *
 *   [ tracking_node (24) ][ refcount_word (8) | typeinfo_ptr (8) ][ payload ]
 *                         ^-- what user/retain/release see as "header"
 *                                              ^-- payload pointer returned
 *
 * The tracking node links every heap allocation into a global
 * doubly-linked list that the sweep walk traverses to find unmarked
 * (unreachable) objects. It also doubles as verifier scratch storage.
 *
 * Static-literal strings (in .rodata) have no tracking prefix because
 * they are never collected. The immortal refcount sentinel
 * short-circuits retain/release before any header access, and the
 * collector skips them by design.
 *
 * Non-atomic refcount: Corvid is single-threaded today.
 */

#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#if defined(_MSC_VER)
#pragma intrinsic(_ReturnAddress)
void* _ReturnAddress(void);
#define CORVID_CALLER_PC() _ReturnAddress()
#else
#define CORVID_CALLER_PC() __builtin_return_address(0)
#endif

#define CORVID_HEADER_BYTES 16

#define CORVID_REFCOUNT_IMMORTAL ((long long)INT64_MIN)
#define CORVID_RC_MASK 0x1FFFFFFFFFFFFFFFLL
#define CORVID_MARK_BIT (1LL << 61)

typedef struct corvid_typeinfo {
    uint32_t size;
    uint32_t flags;
    void (*destroy_fn)(void*);
    void (*trace_fn)(void*, void (*marker)(void*, void*), void*);
    void (*weak_fn)(void*);
    const struct corvid_typeinfo* elem_typeinfo;
    const char* name;
} corvid_typeinfo;

#define CORVID_TI_CYCLIC_CAPABLE 0x01u
#define CORVID_TI_HAS_WEAK_REFS 0x02u
#define CORVID_TI_IS_LIST 0x04u
#define CORVID_TI_LINEAR_CAPABLE 0x08u
#define CORVID_TI_REGION_ALLOCATABLE 0x10u
#define CORVID_TI_REUSE_SHAPE_HINT 0x20u

typedef struct {
    long long refcount_word;
    const corvid_typeinfo* typeinfo;
} corvid_header;

typedef struct corvid_tracking_node {
    struct corvid_tracking_node* next;
    struct corvid_tracking_node* prev;
    const void* last_retain_pc;
    const void* last_release_pc;
    unsigned long long verify_epoch;
    unsigned long long verify_expected_tagged;
} corvid_tracking_node;

#define CORVID_TRACKING_BYTES (sizeof(corvid_tracking_node))

corvid_tracking_node* corvid_live_head = NULL;

/*
 * Fixed-size pooling for small typed blocks.
 *
 * This is intentionally narrow:
 * - only payload sizes that exactly match `typeinfo->size`
 * - only payload sizes up to CORVID_POOL_MAX_PAYLOAD_BYTES
 * - variable-sized payloads still go through malloc/free
 */
#define CORVID_POOL_BUCKET_STRIDE 8
#define CORVID_POOL_MAX_PAYLOAD_BYTES 256
#define CORVID_POOL_BUCKETS \
    (CORVID_POOL_MAX_PAYLOAD_BYTES / CORVID_POOL_BUCKET_STRIDE)
#define CORVID_POOL_MAX_CACHED_BYTES_PER_BUCKET (2 * 1024 * 1024)

static corvid_tracking_node* corvid_pool_heads[CORVID_POOL_BUCKETS] = {0};
static long long corvid_pool_cached_counts[CORVID_POOL_BUCKETS] = {0};

static size_t corvid_pool_bucket_index(size_t payload_bytes) {
    if (payload_bytes == 0 || payload_bytes > CORVID_POOL_MAX_PAYLOAD_BYTES
        || (payload_bytes % CORVID_POOL_BUCKET_STRIDE) != 0) {
        return (size_t)-1;
    }
    return (payload_bytes / CORVID_POOL_BUCKET_STRIDE) - 1;
}

static long long corvid_pool_cached_cap_for_payload_bytes(size_t payload_bytes) {
    size_t bucket = corvid_pool_bucket_index(payload_bytes);
    if (bucket == (size_t)-1) return 0;

    size_t block_bytes = CORVID_TRACKING_BYTES + CORVID_HEADER_BYTES + payload_bytes;
    if (block_bytes == 0) return 0;

    long long cap =
        (long long)(CORVID_POOL_MAX_CACHED_BYTES_PER_BUCKET / block_bytes);
    return cap > 0 ? cap : 1;
}

static corvid_tracking_node* corvid_pool_take(long long payload_bytes,
                                              const corvid_typeinfo* typeinfo) {
    if (typeinfo == NULL || typeinfo->size == 0) return NULL;
    if ((long long)typeinfo->size != payload_bytes) return NULL;

    size_t bucket = corvid_pool_bucket_index((size_t)payload_bytes);
    if (bucket == (size_t)-1) return NULL;

    corvid_tracking_node* node = corvid_pool_heads[bucket];
    if (node != NULL) {
        corvid_pool_heads[bucket] = node->next;
        corvid_pool_cached_counts[bucket]--;
        node->next = NULL;
        node->prev = NULL;
    }
    return node;
}

static void corvid_pool_put(corvid_tracking_node* node,
                            const corvid_typeinfo* typeinfo) {
    if (typeinfo == NULL || typeinfo->size == 0) {
        free((void*)node);
        return;
    }

    size_t bucket = corvid_pool_bucket_index(typeinfo->size);
    if (bucket == (size_t)-1) {
        free((void*)node);
        return;
    }

    if (corvid_pool_cached_counts[bucket]
        >= corvid_pool_cached_cap_for_payload_bytes(typeinfo->size)) {
        free((void*)node);
        return;
    }

    node->prev = NULL;
    node->next = corvid_pool_heads[bucket];
    corvid_pool_heads[bucket] = node;
    corvid_pool_cached_counts[bucket]++;
}

long long corvid_pool_cached_blocks_for_size(long long payload_bytes) {
    if (payload_bytes <= 0) return 0;
    size_t bucket = corvid_pool_bucket_index((size_t)payload_bytes);
    if (bucket == (size_t)-1) return 0;
    return corvid_pool_cached_counts[bucket];
}

long long corvid_pool_cached_cap_for_size(long long payload_bytes) {
    if (payload_bytes <= 0) return 0;
    return corvid_pool_cached_cap_for_payload_bytes((size_t)payload_bytes);
}

extern void corvid_weak_clear_self(void* payload);

static void corvid_trace_String_fn(void* payload,
                                   void (*marker)(void*, void*),
                                   void* ctx) {
    (void)payload;
    (void)marker;
    (void)ctx;
}

const corvid_typeinfo corvid_typeinfo_String = {
    .size = 0,
    .flags = 0,
    .destroy_fn = NULL,
    .trace_fn = corvid_trace_String_fn,
    .weak_fn = corvid_weak_clear_self,
    .elem_typeinfo = NULL,
    .name = "String",
};

long long corvid_alloc_count = 0;
long long corvid_release_count = 0;
long long corvid_retain_call_count = 0;
long long corvid_release_call_count = 0;

long long corvid_safepoint_count = 0;

void corvid_safepoint_notify(void) { corvid_safepoint_count++; }

long long corvid_gc_trigger_threshold = 0;
static long long corvid_allocs_since_gc = 0;

void corvid_gc(void);

void* corvid_alloc_typed(long long payload_bytes, const corvid_typeinfo* typeinfo) {
    if (payload_bytes < 0) {
        fprintf(stderr,
                "corvid: corvid_alloc_typed called with negative size %lld\n",
                payload_bytes);
        exit(1);
    }
    if (typeinfo == NULL) {
        fprintf(stderr, "corvid: corvid_alloc_typed called with NULL typeinfo\n");
        exit(1);
    }

    size_t total = CORVID_TRACKING_BYTES + CORVID_HEADER_BYTES + (size_t)payload_bytes;
    corvid_tracking_node* node = corvid_pool_take(payload_bytes, typeinfo);
    char* raw = (char*)node;

    if (raw == NULL) {
        raw = (char*)malloc(total);
        if (raw == NULL) {
            fprintf(stderr, "corvid: out of memory (requested %lld bytes)\n",
                    payload_bytes);
            exit(1);
        }
        node = (corvid_tracking_node*)raw;
    }

    corvid_header* h = (corvid_header*)(raw + CORVID_TRACKING_BYTES);
    h->refcount_word = 1;
    h->typeinfo = typeinfo;

    node->last_retain_pc = CORVID_CALLER_PC();
    node->last_release_pc = NULL;
    node->verify_epoch = 0;
    node->verify_expected_tagged = 0;

    node->next = corvid_live_head;
    node->prev = NULL;
    if (corvid_live_head != NULL) {
        corvid_live_head->prev = node;
    }
    corvid_live_head = node;

    corvid_alloc_count++;
    corvid_allocs_since_gc++;

    if (corvid_gc_trigger_threshold > 0
        && corvid_allocs_since_gc >= corvid_gc_trigger_threshold) {
        corvid_allocs_since_gc = 0;
        corvid_gc();
    }

    return (void*)((char*)h + CORVID_HEADER_BYTES);
}

void corvid_free_block(corvid_header* h) {
    corvid_tracking_node* node =
        (corvid_tracking_node*)((char*)h - CORVID_TRACKING_BYTES);
    if (node->prev != NULL) {
        node->prev->next = node->next;
    } else {
        corvid_live_head = node->next;
    }
    if (node->next != NULL) {
        node->next->prev = node->prev;
    }
    corvid_pool_put(node, h->typeinfo);
}

void corvid_retain(void* payload) {
    const void* caller_pc = CORVID_CALLER_PC();
    corvid_retain_call_count++;
    if (payload == NULL) return;

    corvid_header* h = (corvid_header*)((char*)payload - CORVID_HEADER_BYTES);
    if (h->refcount_word == CORVID_REFCOUNT_IMMORTAL) return;

    h->refcount_word++;
    corvid_tracking_node* node =
        (corvid_tracking_node*)((char*)h - CORVID_TRACKING_BYTES);
    node->last_retain_pc = caller_pc;
}

void corvid_release(void* payload) {
    const void* caller_pc = CORVID_CALLER_PC();
    corvid_release_call_count++;
    if (payload == NULL) return;

    corvid_header* h = (corvid_header*)((char*)payload - CORVID_HEADER_BYTES);
    if (h->refcount_word == CORVID_REFCOUNT_IMMORTAL) return;

    corvid_tracking_node* node =
        (corvid_tracking_node*)((char*)h - CORVID_TRACKING_BYTES);
    node->last_release_pc = caller_pc;

    long long previous = h->refcount_word;
    h->refcount_word = previous - 1;
    long long prev_rc = previous & CORVID_RC_MASK;

    if (prev_rc == 1) {
        if (h->typeinfo != NULL && h->typeinfo->weak_fn != NULL) {
            h->typeinfo->weak_fn(payload);
        }
        if (h->typeinfo != NULL && h->typeinfo->destroy_fn != NULL) {
            h->typeinfo->destroy_fn(payload);
        }
        corvid_release_count++;
        corvid_free_block(h);
    } else if (prev_rc <= 0) {
        fprintf(stderr,
                "corvid: corvid_release on already-freed allocation (refcount was %lld)\n",
                prev_rc);
        exit(1);
    }
}
