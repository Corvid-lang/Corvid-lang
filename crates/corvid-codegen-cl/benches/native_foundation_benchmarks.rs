//! Native-tier close-out benchmarks for the early compiler/runtime baseline.
//!
//! Three representative Corvid programs, each measured on both tiers:
//! the tree-walking interpreter (`corvid-vm::run_agent`) and the native
//! AOT binary produced by this crate's codegen.
//!
//! Why these three? Each exercises a distinct codepath from the early
//! native tier, so the per-workload ratio tells us which lowering path is
//! hot or cold:
//!
//!   arith_loop         — Int arithmetic + `for` + List<Int> (12a, 12h)
//!   string_concat_loop — refcount alloc + corvid_string_concat  (12e, 12f)
//!   struct_access_loop — struct alloc + field access + destructor (12g)
//!
//! The measurement is end-to-end wall-clock: for native that includes
//! process spawn + execution + exit, because that's what `corvid run`
//! users actually pay. If native can't beat the interpreter despite
//! carrying the spawn tax, that's a real regression — the native tier stays
//! open until it's fixed (see the fair-comparison gate in
//! ROADMAP.md).
//!
//! The programs are generated at bench-setup time (not hand-written)
//! so the workload size is a single source-of-truth constant per bench,
//! and the resulting `.cor` source is rendered deterministically.

use corvid_codegen_cl::build_native_to_disk;
use corvid_ir::{lower, IrFile};
use corvid_resolve::resolve;
use corvid_runtime::{ProgrammaticApprover, Runtime};
use corvid_syntax::{lex, parse_file};
use corvid_types::typecheck;
use corvid_vm::run_agent;
use criterion::{criterion_group, criterion_main, Criterion};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

// ------------------------------------------------------------
// Workload sizes.
//
// Each program runs an INNER workload OUTER_REPS times per invocation.
// This amortises the ~10 ms process-spawn cost on Windows (native's
// fixed overhead vs. the interpreter's in-process runtime) across
// enough real compute that the per-tier numbers reflect codegen quality
// rather than OS cost.
//
// Inner list is hoisted into a local so both tiers allocate it once per
// invocation (not once per outer iteration) — matches how real agent
// code works and keeps the comparison fair.
// ------------------------------------------------------------

const ARITH_INNER: usize = 200; // inner list length — summed each rep
const ARITH_OUTER: usize = 2500; // 2500 × 200 = 500k arithmetic ops per run

const STRING_INNER: usize = 50; // 50 concats per rep
const STRING_OUTER: usize = 1000; // 1000 × 50 = 50k concat ops per run

const STRUCT_INNER: usize = 100; // 100 struct allocs + field reads per rep
const STRUCT_OUTER: usize = 1000; // 1000 × 100 = 100k struct ops per run

// ------------------------------------------------------------
// Corvid source generators. Each returns a well-typed program whose
// entry agent runs an outer repetition loop around a measured inner
// workload. Inner data is hoisted above the outer loop so allocation
// is paid once, matching realistic agent code shape.
// ------------------------------------------------------------

fn arith_loop_src(inner: usize, outer: usize) -> String {
    let inner_list: Vec<String> = (1..=inner).map(|i| i.to_string()).collect();
    let outer_list: Vec<String> = (0..outer).map(|i| i.to_string()).collect();
    format!(
        "agent main() -> Int:\n    items = [{}]\n    reps = [{}]\n    grand = 0\n    for r in reps:\n        total = 0\n        for x in items:\n            total = total + x\n        grand = grand + total\n    return grand\n",
        inner_list.join(", "),
        outer_list.join(", ")
    )
}

fn string_concat_loop_src(inner: usize, outer: usize) -> String {
    let inner_strs: Vec<String> = (0..inner).map(|i| format!("\"s{i}\"")).collect();
    let outer_list: Vec<String> = (0..outer).map(|i| i.to_string()).collect();
    format!(
        "agent main() -> Int:\n    strs = [{}]\n    reps = [{}]\n    grand = 0\n    for r in reps:\n        count = 0\n        for s in strs:\n            prefixed = \"hi \" + s\n            count = count + 1\n        grand = grand + count\n    return grand\n",
        inner_strs.join(", "),
        outer_list.join(", ")
    )
}

fn struct_access_loop_src(inner: usize, outer: usize) -> String {
    let inner_list: Vec<String> = (1..=inner).map(|i| format!("Amount({i})")).collect();
    let outer_list: Vec<String> = (0..outer).map(|i| i.to_string()).collect();
    format!(
        "type Amount:\n    value: Int\n\nagent main() -> Int:\n    items = [{}]\n    reps = [{}]\n    grand = 0\n    for r in reps:\n        total = 0\n        for a in items:\n            total = total + a.value\n        grand = grand + total\n    return grand\n",
        inner_list.join(", "),
        outer_list.join(", ")
    )
}

// ------------------------------------------------------------
// Helpers shared by every bench.
// ------------------------------------------------------------

fn compile_ir(src: &str) -> IrFile {
    let tokens = lex(src).expect("lex");
    let (file, perr) = parse_file(&tokens);
    assert!(perr.is_empty(), "parse errors: {perr:?}");
    let resolved = resolve(&file);
    assert!(
        resolved.errors.is_empty(),
        "resolve errors: {:?}",
        resolved.errors
    );
    let checked = typecheck(&file, &resolved);
    assert!(
        checked.errors.is_empty(),
        "typecheck errors: {:?}",
        checked.errors
    );
    lower(&file, &resolved, &checked)
}

struct BenchFixture {
    ir: IrFile,
    binary: PathBuf,
    _tmp: tempfile::TempDir,
}

fn build_fixture(name: &str, src: &str) -> BenchFixture {
    let ir = compile_ir(src);
    let tmp = tempfile::Builder::new()
        .prefix("corvid-bench-")
        .tempdir()
        .expect("tempdir");
    let out = tmp.path().join(name);
    // No tools crate linked — bench fixtures are pure-computation.
    let binary = build_native_to_disk(&ir, name, &out, &[]).expect("native build");
    BenchFixture {
        ir,
        binary,
        _tmp: tmp,
    }
}

/// Run both tiers of a given program as a criterion benchmark group.
/// The interpreter path blocks on a single-thread tokio runtime
/// constructed once per bench and reused across samples. The native
/// path spawns the pre-compiled binary; its stdout is discarded.
fn bench_program(c: &mut Criterion, name: &str, src: String) {
    let fixture = build_fixture(name, &src);
    let runtime = Runtime::builder()
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .build();
    let tokio_rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    let mut group = c.benchmark_group(name);

    group.bench_function("interpreter", |b| {
        b.iter(|| {
            tokio_rt
                .block_on(async { run_agent(&fixture.ir, "main", vec![], &runtime).await })
                .expect("interp run")
        })
    });

    group.bench_function("native", |b| {
        b.iter(|| {
            Command::new(&fixture.binary)
                .output()
                .expect("spawn native binary")
        })
    });

    group.finish();
}

// ------------------------------------------------------------
// Benches
// ------------------------------------------------------------

fn bench_arith_loop(c: &mut Criterion) {
    bench_program(c, "arith_loop", arith_loop_src(ARITH_INNER, ARITH_OUTER));
}

fn bench_string_concat_loop(c: &mut Criterion) {
    bench_program(
        c,
        "string_concat_loop",
        string_concat_loop_src(STRING_INNER, STRING_OUTER),
    );
}

fn bench_struct_access_loop(c: &mut Criterion) {
    bench_program(
        c,
        "struct_access_loop",
        struct_access_loop_src(STRUCT_INNER, STRUCT_OUTER),
    );
}

criterion_group!(
    benches,
    bench_arith_loop,
    bench_string_concat_loop,
    bench_struct_access_loop,
);
criterion_main!(benches);
