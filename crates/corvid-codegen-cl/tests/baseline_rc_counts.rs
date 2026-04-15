//! Slice 17b-0 — retain/release call-count baselines.
//!
//! Records the retain/release op counts for a handful of representative
//! Corvid programs under the current codegen-time retain/release
//! insertion model. These numbers are the baseline against which slice
//! 17b-1 (principled dup/drop pass) will be measured.
//!
//! Each baseline is committed as an exact-match assertion. When 17b-1
//! lands, the optimized counts will be lower; the test fails; we
//! update the numbers and the git diff is the receipt of the reduction.
//!
//! Workloads chosen to isolate different RC pressure patterns:
//!
//!   - `string_concat_loop`       — many intermediate String values
//!   - `struct_build_and_destructure` — struct + field access patterns
//!   - `list_of_strings_iter`     — list with refcounted elements
//!   - `passthrough_agent`        — single-parameter agent, no ownership
//!                                  transfer needed beyond callee frame
//!   - `primitive_loop`           — control group: zero refcounted work,
//!                                  should have RETAIN_CALLS=0 and
//!                                  RELEASE_CALLS=0 before AND after.

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

/// Extracted counters from a CORVID_DEBUG_ALLOC run.
#[derive(Debug, PartialEq, Eq)]
struct Counts {
    allocs: i64,
    releases: i64,
    retain_calls: i64,
    release_calls: i64,
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
        retain_calls: pick(stderr, "RETAIN_CALLS="),
        release_calls: pick(stderr, "RELEASE_CALLS="),
    }
}

#[track_caller]
fn run_and_count(src: &str) -> Counts {
    let ir = ir_of(src);
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin_path = tmp.path().join("prog");
    let produced = build_native_to_disk(
        &ir,
        "corvid_baseline_rc",
        &bin_path,
        &[test_tools_lib_path().as_path()],
    )
    .expect("compile + link");
    let output = Command::new(&produced)
        .env("CORVID_DEBUG_ALLOC", "1")
        .output()
        .expect("run compiled binary");
    assert!(
        output.status.success(),
        "compiled binary exited non-zero: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let c = parse_counts(&stderr);
    // Leak-balance invariant — if this ever trips, the counters
    // themselves are broken, not just the reduction target.
    assert_eq!(
        c.allocs, c.releases,
        "baseline run has an RC leak: {c:?}; src=\n{src}"
    );
    c
}

/// Control group — no refcounted types touched. Both baseline AND the
/// post-17b-1 run must have zero retain/release calls. If this ever
/// shows non-zero, the codegen is emitting spurious RC ops on primitive
/// paths and the optimization pass should investigate.
#[test]
fn primitive_loop_has_zero_rc_ops() {
    let src = r#"
agent primitive_loop() -> Int:
    sum = 0
    for i in [1, 2, 3, 4, 5]:
        sum = sum + i * 2
    return sum
"#;
    let c = run_and_count(src);
    // List<Int> allocation itself: 1 alloc for the literal.
    assert_eq!(c.allocs, 1);
    assert_eq!(c.releases, 1);
    // Zero retain/release CALLS even though the list is refcounted —
    // the loop body never retains or releases the list itself (it's
    // borrowed by the for-loop), and Int elements are not refcounted.
    assert_eq!(
        c.retain_calls, 0,
        "primitive loop must have zero retain calls; got {c:?}"
    );
    assert_eq!(
        c.release_calls, 1,
        "primitive loop releases only the list at scope end; got {c:?}"
    );
}

#[test]
fn string_concat_chain_baseline() {
    // Chained concatenations within one expression — exercises
    // intermediate String temporaries and their release at expression
    // end. Loop-based string accumulation isn't a baseline we use yet
    // (mutable-reassignment-of-String in a loop has its own quirks
    // that aren't in scope for 17b's RC analysis).
    let src = r#"
agent string_concat_chain() -> Int:
    s = "a" + "b" + "c" + "d" + "e"
    if s == "abcde":
        return 1
    return 0
"#;
    let c = run_and_count(src);
    eprintln!("BASELINE string_concat_chain: {c:?}");
    assert_eq!(c.allocs, 4, "string_concat_chain allocs");
    // 17b-1b.2 peephole (borrow-at-use-site for string BinOps):
    // the `s == "abcde"` comparison reads `s` directly from its
    // Variable (no ownership-conversion retain) and the eq helper
    // only reads its operands, so the post-op release is skipped
    // for the borrowed operand. Saves 1 retain + 1 release.
    // Pre-17b-1b.2: 1 / 11. Post: 0 / 10.
    assert_eq!(c.retain_calls, 0, "string_concat_chain retain_calls");
    assert_eq!(c.release_calls, 10, "string_concat_chain release_calls");
}

#[test]
fn struct_build_and_destructure_baseline() {
    let src = r#"
type Pair:
    left: String
    right: String

agent struct_build_and_destructure() -> Int:
    p = Pair("hello", "world")
    l = p.left
    r = p.right
    if l == "hello":
        return 1
    return 0
"#;
    let c = run_and_count(src);
    eprintln!("BASELINE struct_build_and_destructure: {c:?}");
    assert_eq!(c.allocs, 1, "struct_build_and_destructure allocs");
    // 17b-1b.2 peephole: `l == "hello"` where l is a bare Local.
    // 17b-1b.3 peephole: `p.left` and `p.right` where p is a bare
    //   Local — FieldAccess target is borrowed, saving 1 retain +
    //   1 release per field access (×2 accesses = 4 ops).
    // Pre-17b-1b: 5 / 9. Post-17b-1b.2: 4 / 8. Post-17b-1b.3: 2 / 6.
    assert_eq!(c.retain_calls, 2, "struct_build_and_destructure retain_calls");
    assert_eq!(c.release_calls, 6, "struct_build_and_destructure release_calls");
}

#[test]
fn list_of_strings_iter_baseline() {
    let src = r#"
agent list_of_strings_iter() -> Int:
    xs = ["alpha", "beta", "gamma"]
    n = 0
    for s in xs:
        if s == "beta":
            n = n + 1
    return n
"#;
    let c = run_and_count(src);
    eprintln!("BASELINE list_of_strings_iter: {c:?}");
    assert_eq!(c.allocs, 1, "list_of_strings_iter allocs");
    // 17b-1b.2 peephole: 3 iterations × `s == "beta"` where s is
    // a bare Local — saves 3 retains + 3 releases.
    // Pre-17b-1b.2: 7 / 15. Post: 4 / 12.
    assert_eq!(c.retain_calls, 4, "list_of_strings_iter retain_calls");
    assert_eq!(c.release_calls, 12, "list_of_strings_iter release_calls");
}

#[test]
fn passthrough_agent_baseline() {
    let src = r#"
agent echo(s: String) -> String:
    return s

agent main() -> Int:
    a = echo("one")
    b = echo("two")
    if a == "one":
        return 1
    return 0
"#;
    let c = run_and_count(src);
    eprintln!("BASELINE passthrough_agent: {c:?}");
    // 17b-1's borrow inference should recognize `echo`'s parameter
    // as borrowed (the body only returns it unchanged — no store,
    // no extra consumers). Expect retain_calls to drop noticeably.
    // 17b-1b.1 (borrow inference for read-only-then-return params):
    // `echo(s)` body is `return s` — no consuming use of s. σ(echo, 0)
    // = Borrowed. Callee skips entry-retain + scope-exit release
    // on s. Caller side unchanged (still retains when producing +1
    // to pass). Result: 2 fewer retains + 2 fewer releases across
    // the 2 echo calls.
    //
    // Pre-17b-1b.1: 5 retain / 8 release.
    // Post-17b-1b.1: committed below.
    assert_eq!(c.allocs, 0, "passthrough_agent allocs (all strings are literals)");
    // 17b-1b.2 peephole: `a == "one"` where a is a bare Local.
    // Saves 1 retain + 1 release.
    // Pre-17b-1b.2: 3 / 6. Post: 2 / 5.
    assert_eq!(c.retain_calls, 2, "passthrough_agent retain_calls");
    assert_eq!(c.release_calls, 5, "passthrough_agent release_calls");
}
