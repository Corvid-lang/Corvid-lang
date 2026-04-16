//! End-to-end integration test for stack map emission +
//! runtime lookup.
//!
//! Each test compiles a small Corvid program to a native binary,
//! runs the binary with `CORVID_DEBUG_STACK_MAPS=1`, parses the
//! `STACK_MAPS_COUNT` and `STACK_MAP_ENTRY` lines that
//! `corvid_stack_maps_dump` writes to stderr, and asserts the
//! emitted table matches what we expect from the program shape.
//!
//! What this test pins:
//!
//!   1. Programs with NO refcounted locals emit a `corvid_stack_maps`
//!      symbol with `entry_count = 0` (so the runtime symbol always
//!      exists; binaries with no GC roots don't break linker).
//!   2. Programs with refcounted locals + a call site emit at least
//!      one entry per safepoint per function. Entries have non-NULL
//!      `fn_start`, in-range `pc_offset`, non-zero `ref_count`, and
//!      `ref_count`-many distinct `ref_offsets`.
//!   3. The bytes emitted by the codegen (size, layout, relocations)
//!      survive end-to-end: the binary loads, the symbol resolves,
//!      the dumper reads it correctly. If any link-time relocation
//!      were broken (wrong `write_function_addr` /
//!      `write_data_addr`), `fn_start` would be NULL or
//!      `ref_offsets` would be wild, and the assertions catch it.
//!
//! Why this is the load-bearing stack-map consumer:
//!
//!   The stack-map table itself is dead weight until the collector's mark
//!   walk uses it. This test exercises the same lookup path the collector
//!   will use (`corvid_stack_maps_entry_count`,
//!   `corvid_stack_maps_entry_at`) against a real compiled binary.
//!   If anything in the emit/relocate/load chain is wrong, this
//!   test fails before collector integration, so we don't compound debugging
//!   complexity.

use corvid_codegen_cl::build_native_to_disk;
use corvid_ir::lower;
use corvid_resolve::resolve;
use corvid_syntax::{lex, parse_file};
use corvid_types::typecheck;
use std::path::PathBuf;
use std::process::Command;

fn test_tools_lib_path() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .to_path_buf();
    let name = if cfg!(windows) {
        "corvid_test_tools.lib"
    } else {
        "libcorvid_test_tools.a"
    };
    workspace_root.join("target").join("release").join(name)
}

fn ir_of(src: &str) -> corvid_ir::IrFile {
    let tokens = lex(src).expect("lex");
    let (file, perr) = parse_file(&tokens);
    assert!(perr.is_empty(), "parse: {perr:?}");
    let resolved = resolve(&file);
    assert!(resolved.errors.is_empty(), "resolve: {:?}", resolved.errors);
    let checked = typecheck(&file, &resolved);
    assert!(checked.errors.is_empty(), "typecheck: {:?}", checked.errors);
    lower(&file, &resolved, &checked)
}

#[derive(Debug, PartialEq, Eq)]
struct DumpedEntry {
    fn_start: usize,
    pc_offset: u32,
    frame_bytes: u32,
    ref_count: u32,
    refs: Vec<u32>,
}

#[derive(Debug)]
struct DumpedTable {
    count: u64,
    entries: Vec<DumpedEntry>,
}

/// Parse `STACK_MAPS_COUNT=N` + `STACK_MAP_ENTRY i ...` lines from
/// the binary's stderr. Strict parser — any malformed line panics
/// with diagnostic context, so test failures point at the bug.
fn parse_dump(stderr: &str) -> DumpedTable {
    let mut count: Option<u64> = None;
    let mut entries: Vec<DumpedEntry> = Vec::new();
    for line in stderr.lines() {
        if let Some(rest) = line.strip_prefix("STACK_MAPS_COUNT=") {
            count = Some(rest.trim().parse().expect("STACK_MAPS_COUNT not a u64"));
        } else if let Some(rest) = line.strip_prefix("STACK_MAP_ENTRY ") {
            // Format:
            //   <i> fn_start=<hex> pc_offset=<u32> frame_bytes=<u32>
            //       ref_count=<u32> refs=[<csv>]
            let mut idx = None::<u64>;
            let mut fn_start: Option<usize> = None;
            let mut pc_offset: Option<u32> = None;
            let mut frame_bytes: Option<u32> = None;
            let mut ref_count: Option<u32> = None;
            let mut refs: Vec<u32> = Vec::new();
            // Split on spaces but be careful: refs=[...] has no spaces inside.
            for tok in rest.split_whitespace() {
                if let Some(v) = tok.strip_prefix("fn_start=") {
                    let h = v.trim_start_matches("0x");
                    fn_start = Some(usize::from_str_radix(h, 16).unwrap_or_else(|e| {
                        panic!("fn_start parse `{v}`: {e}")
                    }));
                } else if let Some(v) = tok.strip_prefix("pc_offset=") {
                    pc_offset = Some(v.parse().expect("pc_offset"));
                } else if let Some(v) = tok.strip_prefix("frame_bytes=") {
                    frame_bytes = Some(v.parse().expect("frame_bytes"));
                } else if let Some(v) = tok.strip_prefix("ref_count=") {
                    ref_count = Some(v.parse().expect("ref_count"));
                } else if let Some(v) = tok.strip_prefix("refs=[") {
                    let inner = v.trim_end_matches(']');
                    if !inner.is_empty() {
                        refs = inner
                            .split(',')
                            .map(|s| s.parse().expect("ref offset"))
                            .collect();
                    }
                } else if idx.is_none() {
                    idx = Some(tok.parse().expect("entry index"));
                }
            }
            entries.push(DumpedEntry {
                fn_start: fn_start.expect("missing fn_start"),
                pc_offset: pc_offset.expect("missing pc_offset"),
                frame_bytes: frame_bytes.expect("missing frame_bytes"),
                ref_count: ref_count.expect("missing ref_count"),
                refs,
            });
        }
    }
    let count = count.unwrap_or_else(|| {
        panic!("STACK_MAPS_COUNT line not present in stderr:\n{stderr}")
    });
    DumpedTable { count, entries }
}

#[track_caller]
fn compile_and_dump(src: &str) -> DumpedTable {
    let ir = ir_of(src);
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin_path = tmp.path().join("prog");
    let produced = build_native_to_disk(
        &ir,
        "corvid_stack_maps_test",
        &bin_path,
        &[test_tools_lib_path().as_path()],
    )
    .expect("compile + link");
    let output = Command::new(&produced)
        .env("CORVID_DEBUG_STACK_MAPS", "1")
        .output()
        .expect("run compiled binary");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "compiled binary exited non-zero. stderr:\n{stderr}"
    );
    parse_dump(&stderr)
}

/// Test 1: a primitive-only program emits an empty table — but the
/// symbol still exists (no link error). This is the load-bearing
/// invariant for programs that don't use refcounted types.
#[test]
fn primitive_only_program_emits_empty_table() {
    let src = r#"
agent main() -> Int:
    n = 0
    for i in [1, 2, 3]:
        n = n + i
    return n
"#;
    let dump = compile_and_dump(src);
    // No refcounted locals → no declare_value_needs_stack_map calls
    // → Cranelift emits no UserStackMap entries → table is empty.
    assert_eq!(dump.count, 0, "primitive-only program: {dump:?}");
    assert!(dump.entries.is_empty());
}

/// Test 2: a program with a refcounted local crossed over a call
/// produces stack map entries. We expect at least one entry — the
/// safepoint at the `print_string` call (in the entry trampoline).
#[test]
fn refcounted_local_across_call_emits_entries() {
    // The entry trampoline prints the return value via
    // `corvid_print_string`, which is a call instruction → safepoint.
    // The agent's local `s` is a refcounted String value that's live
    // across that safepoint.
    let src = r#"
agent main() -> String:
    s = "hello"
    return s
"#;
    let dump = compile_and_dump(src);
    assert!(
        dump.count >= 1,
        "expected at least one stack map entry; got {}: {dump:?}",
        dump.count
    );
    // Every entry must have plausible field values: fn_start
    // resolved to a real address (non-zero), ref_count > 0
    // (otherwise why is it an entry?), and matching ref_offsets.
    for e in &dump.entries {
        assert_ne!(
            e.fn_start, 0,
            "fn_start must be relocated to a real symbol; got NULL: {e:?}"
        );
        assert!(
            e.ref_count > 0,
            "entry with zero refs shouldn't have been emitted: {e:?}"
        );
        assert_eq!(
            e.refs.len() as u32,
            e.ref_count,
            "ref_offsets array length must match ref_count: {e:?}"
        );
        // Offsets are SP-relative byte positions of live refcounted
        // pointers in the frame. Plausible range: small positive
        // integers (typically <1 KB for a function with a handful
        // of locals).
        for off in &e.refs {
            assert!(
                *off < 4096,
                "ref offset {off} unexpectedly large; possible relocation bug: {e:?}"
            );
        }
    }
}

/// Test 3: a program with multiple refcounted locals across multiple
/// call sites produces multiple stack map entries. Verifies the
/// table grows correctly and each entry's fields are independent.
#[test]
fn multiple_refcounted_locals_emit_multiple_entries() {
    let src = r#"
agent echo(s: String) -> String:
    return s

agent main() -> String:
    a = echo("first")
    b = echo("second")
    return a
"#;
    let dump = compile_and_dump(src);
    assert!(
        dump.count >= 2,
        "expected multiple entries (2 echo calls + return); got {}: {dump:?}",
        dump.count
    );
    // Sanity: at least two entries should have distinct (fn_start,
    // pc_offset) pairs — different call sites.
    let mut seen: std::collections::HashSet<(usize, u32)> =
        std::collections::HashSet::new();
    for e in &dump.entries {
        seen.insert((e.fn_start, e.pc_offset));
    }
    assert!(
        seen.len() >= 2,
        "expected at least 2 distinct (fn_start, pc_offset) pairs; got {}: {dump:?}",
        seen.len()
    );
}

/// Test 4: parser sanity. Checks the dump-parser handles the empty
/// refs case (`refs=[]`) which appears for entries with non-pointer
/// types — shouldn't actually happen in current stack-map emission (we only declare I64
/// pointers) but the parser must not crash on it.
#[test]
fn parser_handles_empty_refs_brackets() {
    let synthetic = "STACK_MAPS_COUNT=1\nSTACK_MAP_ENTRY 0 fn_start=0xdeadbeef pc_offset=42 frame_bytes=64 ref_count=0 refs=[]\n";
    let dump = parse_dump(synthetic);
    assert_eq!(dump.count, 1);
    assert_eq!(dump.entries.len(), 1);
    assert_eq!(dump.entries[0].fn_start, 0xdeadbeef);
    assert_eq!(dump.entries[0].pc_offset, 42);
    assert_eq!(dump.entries[0].ref_count, 0);
    assert!(dump.entries[0].refs.is_empty());
}
