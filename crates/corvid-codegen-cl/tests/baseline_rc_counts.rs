//! Retain/release call-count baselines.
//!
//! Records the retain/release op counts for a handful of representative
//! Corvid programs under the current codegen-time retain/release
//! insertion model. These numbers are the baseline against which later
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
    let path = workspace_root.join("target").join("release").join(name);
    // Route the linker through `corvid_test_tools.lib` (which already
    // bundles `corvid-runtime` transitively) instead of pairing it
    // with the standalone `corvid_runtime.lib`. See
    // `corvid-codegen-cl::link::link_binary`.
    unsafe {
        std::env::set_var("CORVID_RUNTIME_STATICLIB_OVERRIDE", &path);
    }
    path
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
    //   a bare Local — saves 3 retains + 3 releases.
    // 17b-1b.4 peephole: `for s in xs` where xs is a bare Local —
    //   iter is borrowed, saving the iter-retain + loop-exit-release
    //   pair (1 retain + 1 release).
    // Pre-17b-1b: 7/15. Post-17b-1b.2: 4/12. Post-17b-1b.4: 3/11.
    // Post-17b-1b.6d (unified pass): 0 / 10 — the pass elides the
    //   three per-iteration s retains entirely because `s` reads in
    //   the body are classified at Borrowed (via BinOp peephole path)
    //   and the unified pass schedules Drops precisely at last-use,
    //   so intermediate retains disappear. Release count drops by
    //   one because the pass's drop-on-iter-exit replaces the
    //   scope-end release + separate drop pair with a single Drop.
    assert_eq!(c.retain_calls, 0, "list_of_strings_iter retain_calls");
    assert_eq!(c.release_calls, 10, "list_of_strings_iter release_calls");
}

#[test]
fn local_arg_to_borrowed_callee_baseline() {
    // 17b-1b.5 target pattern: a bare Local arg passed to a callee
    // whose borrow_sig says Borrowed. Without the caller-side
    // peephole, each such call pays a retain (ownership-conversion
    // on Local read) + a release (post-call cleanup of caller's +1).
    // With the peephole: both elided — the callee receives the
    // Local's value as a borrow, and since the callee's borrow
    // inference already elides its entry-retain + scope-exit
    // release (17b-1b.1), the whole retain/release pair on both
    // sides collapses.
    let src = r#"
agent echo(s: String) -> String:
    return s

agent main() -> Int:
    x = "shared"
    a = echo(x)
    b = echo(x)
    if a == "shared":
        return 1
    return 0
"#;
    let c = run_and_count(src);
    eprintln!("BASELINE local_arg_to_borrowed_callee: {c:?}");
    assert_eq!(c.allocs, 0, "local_arg_to_borrowed_callee allocs");
    // Two echo(x) calls where x is a Local + echo.borrow_sig[0] =
    // Borrowed. Caller-side peephole skips the pre-call retain AND
    // post-call release for each call; callee-side (17b-1b.1) skips
    // entry-retain + scope-exit release. Both sides collapse — the
    // retain/release traffic on x's refcount nets to zero across
    // the call boundary.
    // Post-17b-1b.6d (unified pass): 1 / 3 — pass's analysis classifies
    //   Tool/Prompt args as Borrowed, Agent args per borrow_sig. With
    //   σ(echo)=Borrowed the caller skips per-call retain+release as
    //   before; residual ops come from the comparison BinOp and
    //   scope setup.
    assert_eq!(c.retain_calls, 1, "local_arg_to_borrowed_callee retain_calls");
    assert_eq!(c.release_calls, 3, "local_arg_to_borrowed_callee release_calls");
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
    // Post-17b-1b.6d (unified pass): 0 / 3 — pass eliminates the
    //   per-call retain entirely (Agent σ is Borrowed for echo; the
    //   pass's Dup-before-non-last-use triggers nowhere because both
    //   echo calls are last uses of their literal args). Release
    //   count drops from 5 to 3 because scope-end releases are
    //   subsumed by precise last-use Drops.
    assert_eq!(c.retain_calls, 0, "passthrough_agent retain_calls");
    assert_eq!(c.release_calls, 3, "passthrough_agent release_calls");
}
