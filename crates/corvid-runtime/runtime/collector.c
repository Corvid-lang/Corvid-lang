/* Corvid native runtime: mark-sweep cycle collector.
 *
 * Mark-sweep cycle collector over the refcount heap. Fires when
 * allocation pressure crosses the threshold set by `CORVID_GC_TRIGGER`
 * (default 10_000 allocations), or when user/test code calls
 * `corvid_gc()` directly.
 *
 * Complements the refcount fast path: acyclic data is freed
 * deterministically by `corvid_release` as soon as refcount hits 0.
 * The collector exists only to catch cycles - refs that form a loop
 * and keep each other alive even though no external reference exists.
 *
 * Dependencies:
 * - Heap header with typeinfo pointer + mark bit slot in refcount word
 * - `corvid_stack_maps` symbol + `corvid_stack_maps_find(pc)`
 * - Cranelift `preserve_frame_pointers` for RBP-chained stack walking
 */

#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>

#if defined(_MSC_VER)
/* MSVC intrinsic to get the address of the current function's
 * return address. Subtracting 8 gives the saved RBP address on
 * x64 Windows. */
#pragma intrinsic(_AddressOfReturnAddress)
void* _AddressOfReturnAddress(void);
#endif

/* ---- types (must match alloc.c exactly) ---------------------------- */

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

extern corvid_tracking_node* corvid_live_head;
extern long long corvid_release_count;

extern void corvid_free_block(corvid_header* h);

typedef struct corvid_stack_map_entry {
    const void* fn_start;
    uint32_t pc_offset;
    uint32_t frame_bytes;
    uint32_t ref_count;
    uint32_t _pad;
    const uint32_t* ref_offsets;
} corvid_stack_map_entry;

extern const corvid_stack_map_entry* corvid_stack_maps_find(const void* return_pc);

extern int corvid_gc_verify_mode;
extern void corvid_gc_verify(void** roots, size_t n_roots);

typedef struct {
    long long alloc_count;
    long long safepoint_count;
    long long cycle_index;
} corvid_gc_trigger_record;

extern long long corvid_alloc_count;
extern long long corvid_safepoint_count;

#define CORVID_GC_TRIGGER_LOG_CAP 1024
static corvid_gc_trigger_record corvid_gc_trigger_log[CORVID_GC_TRIGGER_LOG_CAP];
static long long corvid_gc_trigger_log_len = 0;
static long long corvid_gc_cycle_count = 0;
static uint64_t corvid_gc_total_ns_total = 0;
static uint64_t corvid_gc_mark_count_total = 0;
static uint64_t corvid_gc_sweep_count_total = 0;
static uint64_t corvid_gc_cycle_reclaimed_total = 0;
static uint64_t corvid_gc_mark_count_current = 0;
static uint64_t corvid_gc_sweep_count_current = 0;
static uint64_t corvid_gc_cycle_reclaimed_current = 0;

long long corvid_gc_trigger_log_length(void) { return corvid_gc_trigger_log_len; }
uint64_t corvid_gc_total_ns(void) { return corvid_gc_total_ns_total; }
uint64_t corvid_gc_mark_count(void) { return corvid_gc_mark_count_total; }
uint64_t corvid_gc_sweep_count(void) { return corvid_gc_sweep_count_total; }
uint64_t corvid_gc_cycle_reclaimed_count(void) { return corvid_gc_cycle_reclaimed_total; }

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

int corvid_gc_trigger_log_at(long long index,
                             long long* out_alloc,
                             long long* out_safepoint,
                             long long* out_cycle) {
    if (index < 0 || index >= corvid_gc_trigger_log_len) return 0;
    const corvid_gc_trigger_record* r = &corvid_gc_trigger_log[index];
    if (out_alloc) *out_alloc = r->alloc_count;
    if (out_safepoint) *out_safepoint = r->safepoint_count;
    if (out_cycle) *out_cycle = r->cycle_index;
    return 1;
}

static void corvid_gc_record_trigger(void) {
    if (corvid_gc_trigger_log_len >= CORVID_GC_TRIGGER_LOG_CAP) return;
    corvid_gc_trigger_record* r = &corvid_gc_trigger_log[corvid_gc_trigger_log_len++];
    r->alloc_count = corvid_alloc_count;
    r->safepoint_count = corvid_safepoint_count;
    r->cycle_index = corvid_gc_cycle_count++;
}

/* ---- frame-pointer walk -------------------------------------------- */

static void* corvid_gc_capture_rbp(void) {
#if defined(_MSC_VER)
    return (char*)_AddressOfReturnAddress() - 8;
#else
    return __builtin_frame_address(0);
#endif
}

/* ---- mark walk ---------------------------------------------------- */

static int corvid_mark_block(corvid_header* h) {
    if (h->refcount_word == CORVID_REFCOUNT_IMMORTAL) return 0;
    if ((h->refcount_word & CORVID_MARK_BIT) != 0) return 0;
    h->refcount_word |= CORVID_MARK_BIT;
    return 1;
}

static void corvid_gc_mark_marker(void* payload, void* ctx) {
    (void)ctx;
    if (payload == NULL) return;
    corvid_header* h = (corvid_header*)((char*)payload - CORVID_HEADER_BYTES);
    if (!corvid_mark_block(h)) return;
    corvid_gc_mark_count_current++;
    if (h->typeinfo != NULL && h->typeinfo->trace_fn != NULL) {
        h->typeinfo->trace_fn(payload, corvid_gc_mark_marker, NULL);
    }
}

static void corvid_gc_mark_stack(void* base_rbp) {
    void** frame = (void**)base_rbp;
    void** prev_frame = NULL;

    const int FRAME_LIMIT = 256;
    const uintptr_t STACK_RANGE_BYTES = 2 * 1024 * 1024;
    const uintptr_t base_addr = (uintptr_t)base_rbp;
    int frame_count = 0;

    while (frame != NULL && frame_count < FRAME_LIMIT) {
        frame_count++;
        if (((uintptr_t)frame & 0x7) != 0) break;
        if (prev_frame != NULL && frame <= prev_frame) break;

        uintptr_t diff = (uintptr_t)frame > base_addr
                             ? (uintptr_t)frame - base_addr
                             : base_addr - (uintptr_t)frame;
        if (diff > STACK_RANGE_BYTES) break;

        void* return_pc = frame[1];
        void* saved_rbp = frame[0];
        if ((uintptr_t)return_pc < 0x1000) break;

        void* sp_at_call = (char*)frame + 16;
        const corvid_stack_map_entry* e = corvid_stack_maps_find(return_pc);
        if (e != NULL) {
            for (uint32_t i = 0; i < e->ref_count; i++) {
                uint32_t offset = e->ref_offsets[i];
                void** slot = (void**)((char*)sp_at_call + offset);
                void* gc_ref = *slot;
                if (gc_ref != NULL) {
                    corvid_gc_mark_marker(gc_ref, NULL);
                }
            }
        }

        prev_frame = frame;
        frame = (void**)saved_rbp;
    }
}

/* ---- sweep walk --------------------------------------------------- */

static void corvid_gc_decrement_marker(void* payload, void* ctx) {
    (void)ctx;
    if (payload == NULL) return;
    corvid_header* h = (corvid_header*)((char*)payload - CORVID_HEADER_BYTES);
    if (h->refcount_word == CORVID_REFCOUNT_IMMORTAL) return;

    long long rc = h->refcount_word & CORVID_RC_MASK;
    long long high = h->refcount_word & ~CORVID_RC_MASK;
    if (rc > 0) {
        h->refcount_word = high | (rc - 1);
    }
}

static void corvid_gc_sweep(void) {
    corvid_tracking_node* node = corvid_live_head;
    while (node != NULL) {
        corvid_header* h = (corvid_header*)((char*)node + CORVID_TRACKING_BYTES);
        if (h->refcount_word != CORVID_REFCOUNT_IMMORTAL
            && (h->refcount_word & CORVID_MARK_BIT) == 0) {
            void* payload = (void*)((char*)h + CORVID_HEADER_BYTES);
            if (h->typeinfo != NULL && h->typeinfo->trace_fn != NULL) {
                h->typeinfo->trace_fn(payload, corvid_gc_decrement_marker, NULL);
            }
        }
        node = node->next;
    }

    node = corvid_live_head;
    while (node != NULL) {
        corvid_tracking_node* next = node->next;
        corvid_header* h = (corvid_header*)((char*)node + CORVID_TRACKING_BYTES);
        corvid_gc_sweep_count_current++;
        if (h->refcount_word != CORVID_REFCOUNT_IMMORTAL) {
            if ((h->refcount_word & CORVID_MARK_BIT) == 0) {
                if (h->typeinfo != NULL && h->typeinfo->weak_fn != NULL) {
                    h->typeinfo->weak_fn((void*)((char*)h + CORVID_HEADER_BYTES));
                }
                corvid_release_count++;
                corvid_gc_cycle_reclaimed_current++;
                corvid_free_block(h);
            } else {
                h->refcount_word &= ~CORVID_MARK_BIT;
            }
        }
        node = next;
    }
}

/* ---- public entry -------------------------------------------------- */

static int corvid_gc_running = 0;

void corvid_gc(void) {
    if (corvid_gc_running) return;
    corvid_gc_running = 1;
    uint64_t gc_start_ns = corvid_profile_runtime_enabled() ? corvid_now_ns() : 0;
    corvid_gc_mark_count_current = 0;
    corvid_gc_sweep_count_current = 0;
    corvid_gc_cycle_reclaimed_current = 0;

    corvid_gc_record_trigger();

    void* base_rbp = corvid_gc_capture_rbp();
    corvid_gc_mark_stack(base_rbp);

    if (corvid_gc_verify_mode != 0) {
        corvid_gc_verify(NULL, 0);
    }

    corvid_gc_sweep();

    corvid_gc_mark_count_total += corvid_gc_mark_count_current;
    corvid_gc_sweep_count_total += corvid_gc_sweep_count_current;
    corvid_gc_cycle_reclaimed_total += corvid_gc_cycle_reclaimed_current;
    if (gc_start_ns != 0) {
        corvid_gc_total_ns_total += corvid_now_ns() - gc_start_ns;
    }

    corvid_gc_running = 0;
}

void corvid_gc_from_roots(void** roots, size_t n_roots) {
    if (corvid_gc_running) return;
    corvid_gc_running = 1;
    uint64_t gc_start_ns = corvid_profile_runtime_enabled() ? corvid_now_ns() : 0;
    corvid_gc_mark_count_current = 0;
    corvid_gc_sweep_count_current = 0;
    corvid_gc_cycle_reclaimed_current = 0;

    corvid_gc_record_trigger();

    for (size_t i = 0; i < n_roots; i++) {
        if (roots[i] != NULL) {
            corvid_gc_mark_marker(roots[i], NULL);
        }
    }

    if (corvid_gc_verify_mode != 0) {
        corvid_gc_verify(roots, n_roots);
    }

    corvid_gc_sweep();

    corvid_gc_mark_count_total += corvid_gc_mark_count_current;
    corvid_gc_sweep_count_total += corvid_gc_sweep_count_current;
    corvid_gc_cycle_reclaimed_total += corvid_gc_cycle_reclaimed_current;
    if (gc_start_ns != 0) {
        corvid_gc_total_ns_total += corvid_now_ns() - gc_start_ns;
    }

    corvid_gc_running = 0;
}
