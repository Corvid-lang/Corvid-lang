/* Corvid native runtime: stack map lookup.
 *
 * Phase 17c — given a return PC observed in a stack frame during a
 * cycle-collection mark phase, find the `corvid_stack_map_entry`
 * (emitted by codegen into the `.rodata` symbol `corvid_stack_maps`)
 * that describes the live refcounted pointers at that PC.
 *
 * Binary layout of `corvid_stack_maps` — must match the emitter in
 * `crates/corvid-codegen-cl/src/lowering.rs::emit_stack_map_table`:
 *
 *   offset  0 :  u64 entry_count
 *   offset  8 :  u64 reserved (= 0)
 *   offset 16 :  entries[entry_count] — each 32 bytes:
 *                   +0  const void* fn_start     (reloc'd to function symbol)
 *                   +8  u32          pc_offset    (return_pc = fn_start + pc_offset)
 *                  +12  u32          frame_bytes
 *                  +16  u32          ref_count
 *                  +20  u32          _pad
 *                  +24  const u32*   ref_offsets  (reloc'd into refs pool)
 *              refs pool: flat u32 array, each is an SP-relative offset
 *              of a live refcounted pointer at the corresponding
 *              safepoint's PC.
 *
 * Phase 17d will consume this by walking task stacks, looking up
 * each frame's return PC, and for each found entry marking the
 * refcounted pointers at SP + ref_offsets[i].
 *
 * Lookup strategy: linear scan. Acceptable for v0.1 (programs have
 * <1000 entries). Once programs get larger and the mark phase runs
 * frequently enough to matter, upgrade to binary search — the
 * emitter already sorts entries by (fn_id, pc_offset) for
 * determinism, so the table is pre-sorted for binary search over
 * fn_start + pc_offset via any stable tiebreaker.
 */

#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

/* Must match lowering.rs::emit_stack_map_table layout exactly. */
typedef struct corvid_stack_map_entry {
    const void* fn_start;
    uint32_t pc_offset;
    uint32_t frame_bytes;
    uint32_t ref_count;
    uint32_t _pad;
    const uint32_t* ref_offsets;
} corvid_stack_map_entry;

typedef struct corvid_stack_maps_header {
    uint64_t entry_count;
    uint64_t _reserved;
    corvid_stack_map_entry entries[];
} corvid_stack_maps_header;

extern const corvid_stack_maps_header corvid_stack_maps;

/* Given a return PC (typically read from a stack frame during the
 * cycle-collection mark phase), return a pointer to the matching
 * stack map entry, or NULL if the PC does not correspond to any
 * safepoint in any compiled Corvid function.
 *
 * The NULL case is expected and correct: not every PC in the
 * program is a safepoint (non-call instructions, C runtime frames,
 * tokio-internal frames, etc.). 17d's mark phase should treat NULL
 * as "skip this frame, no Corvid roots here."
 */
const corvid_stack_map_entry*
corvid_stack_maps_find(const void* return_pc) {
    const uintptr_t target = (uintptr_t)return_pc;
    const uint64_t n = corvid_stack_maps.entry_count;
    for (uint64_t i = 0; i < n; i++) {
        const corvid_stack_map_entry* e = &corvid_stack_maps.entries[i];
        const uintptr_t entry_pc =
            (uintptr_t)e->fn_start + (uintptr_t)e->pc_offset;
        if (entry_pc == target) {
            return e;
        }
    }
    return NULL;
}

/* Introspection helpers — primarily for 17c's integration test and
 * debug builds. 17d's mark phase uses `corvid_stack_maps_find`
 * directly. */

uint64_t corvid_stack_maps_entry_count(void) {
    return corvid_stack_maps.entry_count;
}

const corvid_stack_map_entry*
corvid_stack_maps_entry_at(uint64_t index) {
    if (index >= corvid_stack_maps.entry_count) {
        return NULL;
    }
    return &corvid_stack_maps.entries[index];
}

/* Debug dumper — prints the entire stack map table to stderr in a
 * grep-friendly format. Used by the 17c integration test (sets
 * `CORVID_DEBUG_STACK_MAPS=1` in the env, runs the compiled binary,
 * parses the stderr lines to assert the table looks correct).
 *
 * Output format (parser must match exactly):
 *
 *   STACK_MAPS_COUNT=<n>
 *   STACK_MAP_ENTRY <i> fn_start=<hex> pc_offset=<u32> frame_bytes=<u32> ref_count=<u32> refs=[<off>,<off>,...]
 *   ...
 *
 * Called by `corvid_init` in `entry.c` when the env var is set, so
 * the dump fires once per binary execution before user code runs.
 */
void corvid_stack_maps_dump(void) {
    uint64_t n = corvid_stack_maps.entry_count;
    fprintf(stderr, "STACK_MAPS_COUNT=%llu\n", (unsigned long long)n);
    for (uint64_t i = 0; i < n; i++) {
        const corvid_stack_map_entry* e = &corvid_stack_maps.entries[i];
        fprintf(stderr,
                "STACK_MAP_ENTRY %llu fn_start=%p pc_offset=%u frame_bytes=%u ref_count=%u refs=[",
                (unsigned long long)i,
                (const void*)e->fn_start,
                (unsigned)e->pc_offset,
                (unsigned)e->frame_bytes,
                (unsigned)e->ref_count);
        for (uint32_t j = 0; j < e->ref_count; j++) {
            if (j > 0) fprintf(stderr, ",");
            fprintf(stderr, "%u", (unsigned)e->ref_offsets[j]);
        }
        fprintf(stderr, "]\n");
    }
}

/* Phase 17d — moved env-var parsing to entry.c to keep getenv out
 * of stack_maps.o. The minimal-CRT ffi_bridge_smoke test links
 * corvid_c_runtime without a full stdlib; pulling in getenv via
 * stack_maps.o would break its link. entry.c's corvid_init sets
 * this flag at program start. */
int corvid_stack_maps_dump_requested = 0;
