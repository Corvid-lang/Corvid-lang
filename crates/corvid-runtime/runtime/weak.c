/* Corvid native runtime: weak references (slice 17g).
 *
 * Weak values are pointer-sized, refcounted "slot boxes":
 *
 *   payload layout:
 *     offset 0: target payload pointer (nullable)
 *     offset 8: side-table node pointer (nullable)
 *
 * The box itself is heap-managed and can be stored in structs / lists
 * like any other refcounted value. It never owns the strong target.
 * `corvid_weak_upgrade` borrows the box, retains the live target if
 * present, and returns either that +1 strong payload pointer or NULL.
 *
 * Side-table design:
 *   - global open-addressed hash map keyed by strong payload pointer
 *   - each bucket owns an intrusive list of external nodes
 *   - resize happens only in `corvid_weak_new`
 *   - clear/free paths never resize
 */

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

typedef struct corvid_typeinfo {
    uint32_t size;
    uint32_t flags;
    void (*destroy_fn)(void*);
    void (*trace_fn)(void*, void (*marker)(void*, void*), void*);
    void (*weak_fn)(void*);
    const struct corvid_typeinfo* elem_typeinfo;
    const char* name;
} corvid_typeinfo;

typedef struct corvid_weak_box corvid_weak_box;

typedef struct corvid_weak_node {
    void** slot_addr;
    void* key;
    corvid_weak_box* owner;
    struct corvid_weak_node* next_block;
    struct corvid_weak_node* prev_block;
} corvid_weak_node;

typedef struct corvid_weak_bucket {
    void* key;
    corvid_weak_node* head;
} corvid_weak_bucket;

struct corvid_weak_box {
    void* target;
    corvid_weak_node* node;
};

#define CORVID_WEAK_EMPTY ((void*)0)
#define CORVID_WEAK_TOMBSTONE ((void*)1)

extern void* corvid_alloc_typed(long long payload_bytes, const corvid_typeinfo* typeinfo);
extern void corvid_retain(void* payload);

static corvid_weak_bucket* corvid_weak_buckets = NULL;
static size_t corvid_weak_cap = 0;
static size_t corvid_weak_len = 0;

static void corvid_weak_destroy_box(void* payload);

const corvid_typeinfo corvid_typeinfo_WeakBox = {
    .size = sizeof(corvid_weak_box),
    .flags = 0,
    .destroy_fn = corvid_weak_destroy_box,
    .trace_fn = NULL,
    .weak_fn = NULL,
    .elem_typeinfo = NULL,
    .name = "WeakBox",
};

static uintptr_t corvid_weak_hash_ptr(void* ptr) {
    uintptr_t x = (uintptr_t)ptr;
    x ^= x >> 33;
    x *= (uintptr_t)0xff51afd7ed558ccdULL;
    x ^= x >> 33;
    return x;
}

static corvid_weak_bucket* corvid_weak_find_bucket(void* key) {
    if (key == NULL || corvid_weak_cap == 0) return NULL;
    size_t idx = (size_t)(corvid_weak_hash_ptr(key) % corvid_weak_cap);
    for (size_t probes = 0; probes < corvid_weak_cap; probes++) {
        corvid_weak_bucket* bucket = &corvid_weak_buckets[idx];
        if (bucket->key == CORVID_WEAK_EMPTY) return NULL;
        if (bucket->key == key) return bucket;
        idx = (idx + 1) % corvid_weak_cap;
    }
    return NULL;
}

static corvid_weak_bucket* corvid_weak_find_insert_bucket(void* key) {
    size_t idx = (size_t)(corvid_weak_hash_ptr(key) % corvid_weak_cap);
    corvid_weak_bucket* first_tombstone = NULL;
    for (size_t probes = 0; probes < corvid_weak_cap; probes++) {
        corvid_weak_bucket* bucket = &corvid_weak_buckets[idx];
        if (bucket->key == key) return bucket;
        if (bucket->key == CORVID_WEAK_TOMBSTONE && first_tombstone == NULL) {
            first_tombstone = bucket;
        } else if (bucket->key == CORVID_WEAK_EMPTY) {
            return first_tombstone != NULL ? first_tombstone : bucket;
        }
        idx = (idx + 1) % corvid_weak_cap;
    }
    return first_tombstone;
}

static void corvid_weak_rehash(size_t new_cap) {
    corvid_weak_bucket* old_buckets = corvid_weak_buckets;
    size_t old_cap = corvid_weak_cap;

    corvid_weak_buckets = (corvid_weak_bucket*)calloc(new_cap, sizeof(corvid_weak_bucket));
    if (corvid_weak_buckets == NULL) {
        fprintf(stderr, "corvid: out of memory growing weak side-table\n");
        exit(1);
    }
    corvid_weak_cap = new_cap;
    corvid_weak_len = 0;

    for (size_t i = 0; i < old_cap; i++) {
        corvid_weak_bucket* old = &old_buckets[i];
        if (old->key == CORVID_WEAK_EMPTY || old->key == CORVID_WEAK_TOMBSTONE || old->head == NULL) {
            continue;
        }
        corvid_weak_bucket* fresh = corvid_weak_find_insert_bucket(old->key);
        fresh->key = old->key;
        fresh->head = old->head;
        corvid_weak_len++;
    }

    free(old_buckets);
}

static void corvid_weak_ensure_capacity(void) {
    if (corvid_weak_cap == 0) {
        corvid_weak_rehash(16);
        return;
    }
    if ((corvid_weak_len + 1) * 4 >= corvid_weak_cap * 3) {
        corvid_weak_rehash(corvid_weak_cap * 2);
    }
}

static void corvid_weak_bucket_became_empty(corvid_weak_bucket* bucket) {
    if (bucket->head == NULL && bucket->key != CORVID_WEAK_EMPTY && bucket->key != CORVID_WEAK_TOMBSTONE) {
        bucket->key = CORVID_WEAK_TOMBSTONE;
        if (corvid_weak_len > 0) corvid_weak_len--;
    }
}

static void corvid_weak_unlink_node(corvid_weak_bucket* bucket, corvid_weak_node* node) {
    if (node->prev_block != NULL) {
        node->prev_block->next_block = node->next_block;
    } else if (bucket != NULL) {
        bucket->head = node->next_block;
    }
    if (node->next_block != NULL) {
        node->next_block->prev_block = node->prev_block;
    }
    node->prev_block = NULL;
    node->next_block = NULL;
    if (bucket != NULL) {
        corvid_weak_bucket_became_empty(bucket);
    }
}

void* corvid_weak_new(void* strong_payload) {
    corvid_weak_box* box =
        (corvid_weak_box*)corvid_alloc_typed(sizeof(corvid_weak_box), &corvid_typeinfo_WeakBox);
    box->target = strong_payload;
    box->node = NULL;

    if (strong_payload == NULL) return box;

    corvid_weak_ensure_capacity();
    corvid_weak_bucket* bucket = corvid_weak_find_insert_bucket(strong_payload);
    if (bucket == NULL) {
        fprintf(stderr, "corvid: weak side-table insert failed\n");
        exit(1);
    }
    if (bucket->key == CORVID_WEAK_EMPTY || bucket->key == CORVID_WEAK_TOMBSTONE) {
        bucket->key = strong_payload;
        bucket->head = NULL;
        corvid_weak_len++;
    }

    corvid_weak_node* node = (corvid_weak_node*)malloc(sizeof(corvid_weak_node));
    if (node == NULL) {
        fprintf(stderr, "corvid: out of memory allocating weak side-table node\n");
        exit(1);
    }
    node->slot_addr = &box->target;
    node->key = strong_payload;
    node->owner = box;
    node->prev_block = NULL;
    node->next_block = bucket->head;
    if (bucket->head != NULL) {
        bucket->head->prev_block = node;
    }
    bucket->head = node;
    box->node = node;
    return box;
}

void* corvid_weak_upgrade(void* weak_payload) {
    if (weak_payload == NULL) return NULL;
    corvid_weak_box* box = (corvid_weak_box*)weak_payload;
    void* target = box->target;
    if (target != NULL) {
        corvid_retain(target);
    }
    return target;
}

static void corvid_weak_destroy_box(void* payload) {
    corvid_weak_box* box = (corvid_weak_box*)payload;
    corvid_weak_node* node = box->node;
    box->target = NULL;
    if (node == NULL) return;

    corvid_weak_bucket* bucket = corvid_weak_find_bucket(node->key);
    if (node->slot_addr != NULL) {
        *node->slot_addr = NULL;
    }
    box->node = NULL;
    if (bucket != NULL) {
        corvid_weak_unlink_node(bucket, node);
    }
    free(node);
}

void corvid_weak_clear_self(void* strong_payload) {
    corvid_weak_bucket* bucket = corvid_weak_find_bucket(strong_payload);
    if (bucket == NULL) return;

    corvid_weak_node* node = bucket->head;
    while (node != NULL) {
        corvid_weak_node* next = node->next_block;
        if (node->slot_addr != NULL) {
            *node->slot_addr = NULL;
        }
        if (node->owner != NULL) {
            node->owner->node = NULL;
        }
        corvid_weak_unlink_node(bucket, node);
        free(node);
        node = next;
    }
}
