//! Slice 17b-2 — drop specialization (Mojo ASAP).
//!
//! The unified ownership pass (.6d-2) schedules Drops based on
//! per-statement last-use. That handles most cases but misses
//! control-flow-specific patterns where a local is live going into
//! a conditional, dead coming out, but used in only some branches.
//! On paths that skip the using branch, the local never gets
//! dropped.
//!
//! These tests exercise those patterns. Before 17b-2: some of them
//! leak under the unified pass. After 17b-2: all of them are
//! balanced (allocs == releases).
//!
//! Each test uses `CORVID_DEBUG_ALLOC=1` to assert the runtime
//! leak-balance invariant, plus exit status to catch premature-
//! free aborts.

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
struct Counts {
    allocs: i64,
    releases: i64,
}

fn parse_counts(stderr: &str) -> Counts {
    fn pick(s: &str, key: &str) -> i64 {
        s.lines()
            .find_map(|l| l.strip_prefix(key).and_then(|r| r.trim().parse().ok()))
            .unwrap_or_else(|| panic!("missing `{key}N` in stderr:\n{s}"))
    }
    Counts {
        allocs: pick(stderr, "ALLOCS="),
        releases: pick(stderr, "RELEASES="),
    }
}

/// Compile, run with CORVID_DEBUG_ALLOC=1, assert clean exit + balanced counts.
#[track_caller]
fn assert_balanced(label: &str, src: &str) {
    let ir = ir_of(src);
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin_path = tmp.path().join("prog");
    let produced = build_native_to_disk(
        &ir,
        "corvid_drop_specialization",
        &bin_path,
        &[test_tools_lib_path().as_path()],
    )
    .expect("compile + link");
    let output = Command::new(&produced)
        .env("CORVID_DEBUG_ALLOC", "1")
        .output()
        .expect("run compiled binary");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "[{label}] binary exited non-zero.\nstderr:\n{stderr}"
    );
    let c = parse_counts(&stderr);
    assert_eq!(
        c.allocs, c.releases,
        "[{label}] leak-balance broken: {c:?}\nsrc:\n{src}"
    );
}

/// Helper: build a heap-allocated String. String literals are
/// immortal (refcount = INT64_MIN), so they don't exercise the
/// allocate/release path. Using `"a" + "b"` forces `string_concat`
/// to allocate a fresh heap String — which drop specialization
/// must reliably release on every path.

/// Pattern 1: let heap-alloc-string + conditional branch without else,
/// local used only in then. On the path where cond is false, the
/// local must still be dropped before the function returns.
///
/// Pre-17b-2: leaks on the false-cond path. The pass schedules
/// Drop inside the then-branch only.
#[test]
fn local_used_only_in_then_branch_no_else() {
    let src = r#"
agent main() -> Int:
    s = "al" + "located"
    flag = 0
    if flag == 1:
        if s == "allocated":
            return 1
    return 0
"#;
    assert_balanced("local_used_only_in_then_branch_no_else", src);
}

/// Pattern 2: heap-alloc local + if/else where only one branch uses it.
/// Non-using branch must still drop.
#[test]
fn local_used_only_in_one_branch_with_else() {
    let src = r#"
agent main() -> Int:
    s = "al" + "located"
    flag = 0
    if flag == 1:
        if s == "allocated":
            return 1
    else:
        return 2
    return 0
"#;
    assert_balanced("local_used_only_in_one_branch_with_else", src);
}

/// Pattern 3: heap-alloc local used only in the else, not in then.
#[test]
fn local_used_only_in_else_branch() {
    let src = r#"
agent main() -> Int:
    s = "al" + "located"
    flag = 0
    if flag == 1:
        return 1
    else:
        if s == "allocated":
            return 2
    return 0
"#;
    assert_balanced("local_used_only_in_else_branch", src);
}

/// Pattern 4 (control): heap-alloc local used AFTER the conditional
/// too — no branch-specific drop needed, the analysis's straight-line
/// drop-at-last-use covers it. Must continue to work.
#[test]
fn local_used_after_conditional_control() {
    let src = r#"
agent main() -> Int:
    s = "al" + "located"
    flag = 0
    if flag == 1:
        flag = flag + 1
    if s == "allocated":
        return 0
    return 1
"#;
    assert_balanced("local_used_after_conditional_control", src);
}
