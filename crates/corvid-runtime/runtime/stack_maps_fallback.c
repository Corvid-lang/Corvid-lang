/* Corvid native runtime: weak fallback for `corvid_stack_maps`.
 *
 * Phase 17c introduced the `corvid_stack_maps` data symbol, emitted
 * by the codegen (corvid-codegen-cl/src/lowering.rs::
 * emit_stack_map_table) into every compiled Corvid binary's
 * `.rodata`. Runtime code in `stack_maps.c` and `collector.c`
 * references this symbol via `extern`.
 *
 * But `corvid_c_runtime.lib` is ALSO linked by Rust-only test
 * binaries (e.g. `parity.exe`) that don't have any Corvid-
 * compiled code. Without a fallback definition, those binaries
 * fail to link because nothing provides `corvid_stack_maps`.
 *
 * Solution: this file provides a default empty-table definition
 * with the linker attribute that lets a stronger definition
 * (the codegen-emitted one in a real Corvid binary) override it.
 *
 *   - MSVC: `__declspec(selectany)` — linker tolerates multiple
 *     definitions and picks any. The codegen's definition, being
 *     in the main object of the Corvid binary, typically wins.
 *   - GCC/Clang: `__attribute__((weak))` — weak symbol, overridden
 *     by any strong definition.
 *
 * Rust-only tests see entry_count=0 and the collector's mark
 * phase finds no safepoints to walk (correct no-op behavior).
 * Real Corvid binaries get the codegen-emitted table with real
 * data (correct).
 */

#include <stdint.h>

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

#if defined(_MSC_VER)
__declspec(selectany)
const corvid_stack_maps_header corvid_stack_maps = { 0, 0 };
#else
__attribute__((weak))
const corvid_stack_maps_header corvid_stack_maps = { 0, 0 };
#endif
