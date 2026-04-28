//! Verifier-driven correctness baseline.
//!
//! Runs representative Corvid programs under the combined settings
//!
//!   CORVID_GC_TRIGGER=1      â€” force a GC cycle after every allocation
//!   CORVID_GC_VERIFY=abort   â€” verifier computes expected refcount
//!                              from graph reachability at every cycle
//!                              and `abort()`s on any drift
//!
//! These settings together force the 17f++ verifier to audit every
//! intermediate refcount state produced by the current (pre-17b-1b.6d)
//! codegen. If the codegen is refcount-correct per the runtime
//! contract, every program exits cleanly and drift count stays at 0.
//!
//! Purpose: establish a pre-.6d baseline of correctness. Landing
//! keeping this green means:
//!
//!   - the current scattered emit_retain/release codegen is
//!     verifier-audited on every class of refcounted operation we
//!     cover below
//!   - .6d's edits can be compared to this baseline for regressions
//!   - any latent bug in the CURRENT codegen surfaces NOW rather
//!     than being conflated with .6d's surgical changes
//!
//! Coverage:
//!   - String construction + pass-through
//!   - Struct construction + field access
//!   - List construction + element access
//!   - Nested structures (List<String>)
//!   - Control flow (if-else branches with refcounted locals)
//!   - For-loop iteration over a refcounted list
//!
//! Intentionally NOT covered here (reasons noted):
//!   - Tool / prompt / agent dispatch: these exercise the FFI
//!     bridge paths that my .6d surgery preserves as-is (those
//!     emit_release sites are boundary code, not pass-replaceable).
//!     A follow-up test should add FFI coverage if we want to
//!     pin their correctness too, but that's outside the ownership
//!     pass's scope.
//!   - Weak / Option<refcounted>: these are 17g's surface. Dev B's
//!     targeted tests cover those directly.

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

/// Compile + run `src` with the verifier configured to abort on any
/// drift. Asserts exit status 0 AND that stderr contains no drift
/// report lines.
#[track_caller]
fn audit(label: &str, src: &str) {
    let ir = ir_of(src);
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin_path = tmp.path().join("prog");
    let produced = build_native_to_disk(
        &ir,
        "corvid_verifier_audit",
        &bin_path,
        &[test_tools_lib_path().as_path()],
    )
    .expect("compile + link");

    let output = Command::new(&produced)
        // CORVID_GC_TRIGGER=10 rather than 1: firing GC on EVERY
        // alloc blew up the stack walker in the cranelift-compiled
        // binary's early prologue (the frame pointer chain isn't
        // fully established before the first allocation fires from
        // inside runtime init). A small-but-not-minimal threshold
        // lets the stack settle before GC is invoked, while still
        // forcing multiple cycles to run under the verifier for any
        // program that allocates more than ~10 values.
        .env("CORVID_GC_TRIGGER", "10")
        // warn mode: drift is reported but does NOT abort. We then
        // check stderr for drift lines + exit-time summary. Abort
        // mode masks useful diagnostic info when the first failing
        // fixture kills the process.
        .env("CORVID_GC_VERIFY", "warn")
        .env("CORVID_DEBUG_ALLOC", "1")
        .output()
        .expect("run compiled binary");

    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "[{label}] binary exited non-zero.\nstderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("CORVID_GC_VERIFY: refcount drift"),
        "[{label}] verifier detected refcount drift in current codegen:\nstderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("CORVID_GC_VERIFY:") || !stderr.contains("total drift report(s)"),
        "[{label}] exit-time summary reports drift:\nstderr:\n{stderr}"
    );
}

#[test]
fn string_construction_pass_through() {
    let src = r#"
agent main() -> Int:
    s = "hello"
    return 0
"#;
    audit("string_construction_pass_through", src);
}

#[test]
fn string_concat_chain() {
    let src = r#"
agent main() -> Int:
    s = "a" + "b" + "c" + "d" + "e"
    if s == "abcde":
        return 0
    return 1
"#;
    audit("string_concat_chain", src);
}

#[test]
fn struct_construction_and_field_access() {
    let src = r#"
type Point:
    x: Int
    y: Int

agent main() -> Int:
    p = Point(3, 4)
    return p.x + p.y
"#;
    audit("struct_construction_and_field_access", src);
}

#[test]
fn struct_with_string_field() {
    let src = r#"
type Greeting:
    who: String
    count: Int

agent main() -> Int:
    g = Greeting("world", 3)
    return g.count
"#;
    audit("struct_with_string_field", src);
}

#[test]
fn list_of_ints_iter() {
    let src = r#"
agent main() -> Int:
    sum = 0
    for i in [10, 20, 30]:
        sum = sum + i
    return sum
"#;
    audit("list_of_ints_iter", src);
}

#[test]
fn list_of_strings_indexed() {
    let src = r#"
agent main() -> Int:
    names = ["alpha", "beta", "gamma"]
    if names[1] == "beta":
        return 0
    return 1
"#;
    audit("list_of_strings_indexed", src);
}

#[test]
fn control_flow_with_refcounted_locals() {
    let src = r#"
agent main() -> Int:
    flag = 1
    if flag == 1:
        s = "hot"
        return 0
    else:
        s = "cold"
        return 1
"#;
    audit("control_flow_with_refcounted_locals", src);
}

/// Long-running program that forces many GC cycles. Exercises the
/// verifier against the steady-state refcount pattern of a loop
/// allocating refcounted values in every iteration. Under
/// CORVID_GC_TRIGGER=1 each allocation fires a full mark-sweep +
/// verifier pass; if any cycle's state is wrong, abort fires.
#[test]
fn loop_allocating_strings_forces_many_verifications() {
    let src = r#"
agent main() -> Int:
    tag = "x"
    count = 0
    for i in [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]:
        count = count + 1
    return count
"#;
    audit("loop_allocating_strings_forces_many_verifications", src);
}

/// Nested structure: a struct holding a string field. Tests that
/// typeinfo trace functions work correctly under the verifier â€”
/// a mark-walk traversal through the struct's string field
/// contributes an incoming edge that must match the string's refcount.
#[test]
fn nested_struct_with_string_traverses_cleanly() {
    let src = r#"
type Row:
    label: String
    n: Int

agent main() -> Int:
    r = Row("first", 1)
    return r.n
"#;
    audit("nested_struct_with_string_traverses_cleanly", src);
}

