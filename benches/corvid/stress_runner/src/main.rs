#![allow(dead_code)]

use anyhow::{bail, Context, Result};
use corvid_driver::{build_or_get_cached_native, compile_to_ir};
use serde_json::{Map, Value};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

const ALLOCATION_SCALES: &[(i64, usize)] = &[
    (19, 1),
    (100, 5),
    (1000, 50),
    (10000, 500),
    (100000, 5000),
];
const GC_THRESHOLDS: &[i64] = &[100, 1000, 10000, 50000, 0];
const CYCLE_SCALES: &[usize] = &[10, 100, 1000, 10000];

#[derive(Default)]
struct TrialProfile {
    trial_wall_ms: f64,
    prompt_render_ms: f64,
    json_bridge_ms: f64,
    mock_llm_dispatch_ms: f64,
    trial_init_ms: f64,
    trace_overhead_ms: f64,
    rc_release_time_ms: f64,
    allocs: Option<i64>,
    releases: Option<i64>,
    retain_calls: Option<i64>,
    release_calls: Option<i64>,
    gc_trigger_count: Option<i64>,
    gc_total_ms: Option<f64>,
    gc_mark_count: Option<u64>,
    gc_sweep_count: Option<u64>,
    gc_cycle_count: Option<u64>,
    live_peak_objects: Option<i64>,
    safepoint_count: Option<i64>,
    stack_map_entry_count: Option<u64>,
    verify_drift_count: Option<i64>,
}

struct NativeServer {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    stderr: BufReader<ChildStderr>,
}

struct SyntheticWorkload {
    name: String,
    source_path: PathBuf,
    source: String,
    expected_stdout: String,
    gc_trigger_threshold: Option<i64>,
}

#[repr(C)]
struct CorvidTypeinfo {
    size: u32,
    flags: u32,
    destroy_fn: Option<unsafe extern "C" fn(*mut u8)>,
    trace_fn: Option<
        unsafe extern "C" fn(
            *mut u8,
            Option<unsafe extern "C" fn(*mut u8, *mut u8)>,
            *mut u8,
        ),
    >,
    weak_fn: Option<unsafe extern "C" fn(*mut u8)>,
    elem_typeinfo: *const CorvidTypeinfo,
    name: *const u8,
}

unsafe impl Sync for CorvidTypeinfo {}

#[link(name = "corvid_c_runtime", kind = "static")]
extern "C" {
    fn corvid_alloc_typed(payload_bytes: i64, typeinfo: *const CorvidTypeinfo) -> *mut u8;
    fn corvid_release(payload: *mut u8);
    fn corvid_gc_from_roots(roots: *mut *mut u8, n_roots: usize);
    fn corvid_string_from_bytes(bytes: *const u8, length: i64) -> *mut u8;
    fn corvid_string_concat(a_payload: *mut u8, b_payload: *mut u8) -> *mut u8;

    fn corvid_gc_trigger_log_length() -> i64;
    fn corvid_gc_total_ns() -> u64;
    fn corvid_gc_mark_count() -> u64;
    fn corvid_gc_sweep_count() -> u64;
    fn corvid_gc_cycle_reclaimed_count() -> u64;
    fn corvid_live_object_peak() -> i64;
    fn corvid_reset_live_object_peak();
    fn corvid_reset_gc_alloc_counter();

    static corvid_alloc_count: i64;
    static corvid_release_count: i64;
    static corvid_retain_call_count: i64;
    static corvid_release_call_count: i64;
    static mut corvid_gc_trigger_threshold: i64;
}

static BOX_TYPEINFO: CorvidTypeinfo = CorvidTypeinfo {
    size: 8,
    flags: 0,
    destroy_fn: None,
    trace_fn: None,
    weak_fn: None,
    elem_typeinfo: std::ptr::null(),
    name: std::ptr::null(),
};

unsafe extern "C" fn cell_trace(
    payload: *mut u8,
    marker: Option<unsafe extern "C" fn(*mut u8, *mut u8)>,
    ctx: *mut u8,
) {
    let slots = payload as *mut *mut u8;
    for i in 0..2 {
        let ptr = *slots.add(i);
        if !ptr.is_null() {
            if let Some(marker) = marker {
                marker(ptr, ctx);
            }
        }
    }
}

unsafe extern "C" fn cell_destroy(payload: *mut u8) {
    let slots = payload as *mut *mut u8;
    for i in 0..2 {
        let ptr = *slots.add(i);
        if !ptr.is_null() {
            corvid_release(ptr);
        }
    }
}

static CELL_TYPEINFO: CorvidTypeinfo = CorvidTypeinfo {
    size: 16,
    flags: 0x01,
    destroy_fn: Some(cell_destroy),
    trace_fn: Some(cell_trace),
    weak_fn: None,
    elem_typeinfo: std::ptr::null(),
    name: b"Cell\0".as_ptr(),
};

fn main() -> Result<()> {
    std::env::set_var("CORVID_PROFILE_RUNTIME", "1");
    let mut args = std::env::args().skip(1);
    let output_path = PathBuf::from(
        args.next()
            .unwrap_or_else(|| "benches/results/2026-04-17-rc-gc-tuning/raw.jsonl".to_string()),
    );
    let trials = args
        .next()
        .map(|s| s.parse::<usize>().context("invalid trial count"))
        .transpose()?
        .unwrap_or(30);
    let mode = args.next().unwrap_or_else(|| "all".to_string());
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut out = String::new();
    match mode.as_str() {
        "all" => {
            append_records(&mut out, run_allocation_scaling(trials)?)?;
            append_records(&mut out, run_trigger_sensitivity(trials)?)?;
            append_records(&mut out, run_cycle_stress(trials)?)?;
        }
        "alloc" => append_records(&mut out, run_allocation_scaling(trials)?)?,
        "trigger" => append_records(&mut out, run_trigger_sensitivity(trials)?)?,
        "cycle" => append_records(&mut out, run_cycle_stress(trials)?)?,
        other => bail!("unknown stress mode `{other}`"),
    }
    fs::write(output_path, out)?;
    Ok(())
}

fn selected_release_scale() -> Result<Option<i64>> {
    std::env::var("CORVID_STRESS_RELEASE_SCALE")
        .ok()
        .map(|raw| raw.parse::<i64>().context("invalid CORVID_STRESS_RELEASE_SCALE"))
        .transpose()
}

fn selected_gc_threshold() -> Result<Option<i64>> {
    std::env::var("CORVID_STRESS_THRESHOLD")
        .ok()
        .map(|raw| raw.parse::<i64>().context("invalid CORVID_STRESS_THRESHOLD"))
        .transpose()
}

fn selected_cycle_pairs() -> Result<Option<usize>> {
    std::env::var("CORVID_STRESS_CYCLE_PAIRS")
        .ok()
        .map(|raw| raw.parse::<usize>().context("invalid CORVID_STRESS_CYCLE_PAIRS"))
        .transpose()
}

fn append_records(out: &mut String, records: Vec<Value>) -> Result<()> {
    for record in records {
        out.push_str(&serde_json::to_string(&record)?);
        out.push('\n');
    }
    Ok(())
}

fn workspace_root() -> Result<PathBuf> {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .map(Path::to_path_buf)
        .context("workspace root")
}

fn generated_sources_dir(root: &Path) -> PathBuf {
    root.join("benches").join("corvid").join("generated")
}

fn build_tools_lib(root: &Path) -> Result<PathBuf> {
    let manifest = root.join("benches").join("corvid").join("tools").join("Cargo.toml");
    let lib_name = if cfg!(windows) {
        "corvid_bench_tools.lib"
    } else {
        "libcorvid_bench_tools.a"
    };
    let built = root
        .join("benches")
        .join("corvid")
        .join("tools")
        .join("target")
        .join("release")
        .join(lib_name);
    let status = Command::new("cargo")
        .arg("build")
        .arg("--manifest-path")
        .arg(&manifest)
        .arg("--release")
        .status()?;
    if !status.success() {
        bail!("building benchmark tools failed");
    }
    Ok(built)
}

fn ensure_runtime_staticlib(root: &Path) -> Result<()> {
    let lib_name = if cfg!(windows) {
        "corvid_runtime.lib"
    } else {
        "libcorvid_runtime.a"
    };
    let status = Command::new("cargo")
        .arg("build")
        .arg("-p")
        .arg("corvid-runtime")
        .arg("--release")
        .current_dir(root)
        .status()?;
    if !status.success() {
        bail!("building corvid-runtime staticlib failed");
    }
    let produced = root.join("target").join("release").join(lib_name);
    let expected_runtime = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("debug")
        .join(lib_name);
    if produced.exists() {
        if let Some(parent) = expected_runtime.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&produced, &expected_runtime).with_context(|| {
            format!(
                "copy runtime staticlib `{}` -> `{}`",
                produced.display(),
                expected_runtime.display()
            )
        })?;
    }
    let expected_c_runtime = PathBuf::from(corvid_runtime::c_runtime::C_RUNTIME_LIB_PATH);
    let produced_c_runtime = latest_named_file(
        &root.join("target").join("release").join("build"),
        if cfg!(windows) {
            "corvid_c_runtime.lib"
        } else {
            "libcorvid_c_runtime.a"
        },
    )?;
    if produced_c_runtime.exists() {
        if let Some(parent) = expected_c_runtime.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&produced_c_runtime, &expected_c_runtime).with_context(|| {
            format!(
                "copy c runtime staticlib `{}` -> `{}`",
                produced_c_runtime.display(),
                expected_c_runtime.display()
            )
        })?;
    }
    Ok(())
}

fn latest_named_file(root: &Path, name: &str) -> Result<PathBuf> {
    fn walk(dir: &Path, name: &str, best: &mut Option<(std::time::SystemTime, PathBuf)>) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                walk(&path, name, best)?;
                continue;
            }
            if path.file_name().and_then(|n| n.to_str()) != Some(name) {
                continue;
            }
            let modified = entry.metadata()?.modified()?;
            match best {
                Some((best_modified, _)) if modified <= *best_modified => {}
                _ => *best = Some((modified, path)),
            }
        }
        Ok(())
    }

    let mut best = None;
    walk(root, name, &mut best)?;
    best.map(|(_, path)| path)
        .with_context(|| format!("locate `{}` under `{}`", name, root.display()))
}

fn load_list(item_count: usize) -> String {
    (0..item_count)
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn allocation_source(_name: &str, outer_reps: usize) -> String {
    let inner = (0..10)
        .map(|i| format!("\"s{i}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"agent main() -> Int:
    strs = [{inner}]
    reps = [{reps}]
    grand = 0
    for r in reps:
        count = 0
        for s in strs:
            prefixed = "hi " + s
            tagged = prefixed + "!"
            count = count + 1
        grand = grand + count
    return grand
"#,
        inner = inner,
        reps = load_list(outer_reps)
    )
}

fn workload_for_target(root: &Path, target_releases: i64, outer_reps: usize, threshold: Option<i64>) -> Result<SyntheticWorkload> {
    let name = match threshold {
        Some(value) => format!("alloc_pressure_{target_releases}_threshold_{value}"),
        None => format!("alloc_pressure_{target_releases}"),
    };
    let dir = generated_sources_dir(root);
    fs::create_dir_all(&dir)?;
    let source_path = dir.join(format!("{name}.cor"));
    let source = allocation_source(&name, outer_reps);
    fs::write(&source_path, &source)?;
    Ok(SyntheticWorkload {
        name,
        source_path,
        source,
        expected_stdout: (outer_reps * 10).to_string(),
        gc_trigger_threshold: threshold,
    })
}

fn start_native_server(
    root: &Path,
    tools_lib: &Path,
    workload: &SyntheticWorkload,
    _requests: usize,
) -> Result<NativeServer> {
    let ir = compile_to_ir(&workload.source)
        .map_err(|diags| anyhow::anyhow!("compile diagnostics: {}", diags.len()))?;
    let cached =
        build_or_get_cached_native(&workload.source_path, &workload.source, &ir, Some(tools_lib))?;
    let trial_dir = root
        .join("benches")
        .join("corvid")
        .join("out")
        .join("stress")
        .join(&workload.name)
        .join(format!("persistent-{}", std::process::id()));
    fs::create_dir_all(trial_dir.join("target").join("trace"))?;

    let mut command = Command::new(&cached.path);
    command
        .current_dir(&trial_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("CORVID_TRACE_DISABLE", "1")
        .env("CORVID_PROFILE_RUNTIME", "1")
        .env("CORVID_BENCH_SERVER", "1");
    if let Some(threshold) = workload.gc_trigger_threshold {
        command.env("CORVID_GC_TRIGGER", threshold.to_string());
    }

    let mut child = command
        .spawn()
        .with_context(|| format!("spawn persistent `{}`", cached.path.display()))?;
    Ok(NativeServer {
        stdin: child.stdin.take().context("stress stdin")?,
        stdout: BufReader::new(child.stdout.take().context("stress stdout")?),
        stderr: BufReader::new(child.stderr.take().context("stress stderr")?),
        child,
    })
}

fn finish_server(mut server: NativeServer) -> Result<()> {
    drop(server.stdin);
    let status = server.child.wait()?;
    if !status.success() {
        bail!("persistent stress workload exited with {status}");
    }
    Ok(())
}

fn read_stdout_line(reader: &mut BufReader<ChildStdout>) -> Result<String> {
    let mut line = String::new();
    reader.read_line(&mut line)?;
    Ok(line.trim().to_string())
}

fn read_trial_profile(reader: &mut BufReader<ChildStderr>) -> Result<TrialProfile> {
    let mut profile = TrialProfile::default();
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            bail!("stress workload ended before emitting trial profile");
        }
        if let Some(raw) = line.trim().strip_prefix("CORVID_BENCH_TRIAL=") {
            let value: Value = serde_json::from_str(raw).context("parse stress trial profile")?;
            profile.trial_wall_ms = value.get("trial_wall_ms").and_then(Value::as_f64).unwrap_or(0.0);
            profile.prompt_render_ms = value.get("prompt_render_ms").and_then(Value::as_f64).unwrap_or(0.0);
            profile.json_bridge_ms = value.get("json_bridge_ms").and_then(Value::as_f64).unwrap_or(0.0);
            profile.mock_llm_dispatch_ms = value.get("mock_llm_dispatch_ms").and_then(Value::as_f64).unwrap_or(0.0);
            profile.trial_init_ms = value.get("trial_init_ms").and_then(Value::as_f64).unwrap_or(0.0);
            profile.trace_overhead_ms = value.get("trace_overhead_ms").and_then(Value::as_f64).unwrap_or(0.0);
            profile.rc_release_time_ms = value.get("rc_release_time_ms").and_then(Value::as_f64).unwrap_or(0.0);
            profile.allocs = value.get("allocs").and_then(Value::as_i64);
            profile.releases = value.get("releases").and_then(Value::as_i64);
            profile.retain_calls = value.get("retain_calls").and_then(Value::as_i64);
            profile.release_calls = value.get("release_calls").and_then(Value::as_i64);
            profile.gc_trigger_count = value.get("gc_trigger_count").and_then(Value::as_i64);
            profile.gc_total_ms = value.get("gc_total_ms").and_then(Value::as_f64);
            profile.gc_mark_count = value.get("gc_mark_count").and_then(Value::as_u64);
            profile.gc_sweep_count = value.get("gc_sweep_count").and_then(Value::as_u64);
            profile.gc_cycle_count = value.get("gc_cycle_count").and_then(Value::as_u64);
            profile.live_peak_objects = value.get("live_peak_objects").and_then(Value::as_i64);
            profile.safepoint_count = value.get("safepoint_count").and_then(Value::as_i64);
            profile.stack_map_entry_count =
                value.get("stack_map_entry_count").and_then(Value::as_u64);
            profile.verify_drift_count = value.get("verify_drift_count").and_then(Value::as_i64);
            return Ok(profile);
        }
    }
}

fn run_server_trial(
    scenario_kind: &str,
    scenario_name: &str,
    trial_idx: usize,
    target_releases: i64,
    gc_trigger_threshold: Option<i64>,
    server: &mut NativeServer,
    expected_stdout: &str,
) -> Result<Value> {
    writeln!(server.stdin, "{trial_idx}")?;
    server.stdin.flush()?;
    let stdout = read_stdout_line(&mut server.stdout)?;
    let profile = read_trial_profile(&mut server.stderr)?;
    let total_profiled_ms = profile.prompt_render_ms
        + profile.json_bridge_ms
        + profile.mock_llm_dispatch_ms
        + profile.trial_init_ms
        + profile.trace_overhead_ms
        + profile.rc_release_time_ms
        + profile.gc_total_ms.unwrap_or(0.0);
    let orchestration_ms = profile.trial_wall_ms;
    let unattributed_ms = (orchestration_ms - total_profiled_ms).max(0.0);

    let mut record = Map::new();
    record.insert("kind".into(), Value::String(scenario_kind.into()));
    record.insert("scenario".into(), Value::String(scenario_name.into()));
    record.insert("stack".into(), Value::String("corvid".into()));
    record.insert("process_mode".into(), Value::String("persistent".into()));
    record.insert("trial_idx".into(), Value::from(trial_idx as u64));
    record.insert("wall_ms".into(), Value::from(profile.trial_wall_ms));
    record.insert("external_wait_ms".into(), Value::from(0.0));
    record.insert("actual_external_wait_ms".into(), Value::from(0.0));
    record.insert("orchestration_ms".into(), Value::from(orchestration_ms));
    record.insert("success".into(), Value::Bool(stdout == expected_stdout));
    record.insert("target_release_scale".into(), Value::from(target_releases));
    record.insert(
        "gc_trigger_threshold".into(),
        gc_trigger_threshold.map(Value::from).unwrap_or(Value::Null),
    );
    record.insert("allocs".into(), profile.allocs.map(Value::from).unwrap_or(Value::Null));
    record.insert("releases".into(), profile.releases.map(Value::from).unwrap_or(Value::Null));
    record.insert(
        "rc_retain_count".into(),
        profile.retain_calls.map(Value::from).unwrap_or(Value::Null),
    );
    record.insert(
        "rc_release_count".into(),
        profile.release_calls.map(Value::from).unwrap_or(Value::Null),
    );
    record.insert(
        "gc_trigger_count".into(),
        profile.gc_trigger_count.map(Value::from).unwrap_or(Value::Null),
    );
    record.insert(
        "gc_total_ms".into(),
        profile.gc_total_ms.map(Value::from).unwrap_or(Value::Null),
    );
    record.insert(
        "gc_mark_count".into(),
        profile.gc_mark_count.map(Value::from).unwrap_or(Value::Null),
    );
    record.insert(
        "gc_sweep_count".into(),
        profile.gc_sweep_count.map(Value::from).unwrap_or(Value::Null),
    );
    record.insert(
        "gc_cycle_count".into(),
        profile.gc_cycle_count.map(Value::from).unwrap_or(Value::Null),
    );
    record.insert(
        "peak_live_objects".into(),
        profile.live_peak_objects.map(Value::from).unwrap_or(Value::Null),
    );
    record.insert("prompt_render_ms".into(), Value::from(profile.prompt_render_ms));
    record.insert("json_bridge_ms".into(), Value::from(profile.json_bridge_ms));
    record.insert(
        "mock_llm_dispatch_ms".into(),
        Value::from(profile.mock_llm_dispatch_ms),
    );
    record.insert("trial_init_ms".into(), Value::from(profile.trial_init_ms));
    record.insert("trace_overhead_ms".into(), Value::from(profile.trace_overhead_ms));
    record.insert("rc_release_time_ms".into(), Value::from(profile.rc_release_time_ms));
    record.insert("total_profiled_ms".into(), Value::from(total_profiled_ms));
    record.insert("unattributed_ms".into(), Value::from(unattributed_ms));
    Ok(Value::Object(record))
}

fn snapshot_runtime_counters() -> (i64, i64, i64, i64, i64, u64, u64, u64, u64, i64) {
    unsafe {
        corvid_reset_live_object_peak();
        (
            corvid_alloc_count,
            corvid_release_count,
            corvid_retain_call_count,
            corvid_release_call_count,
            corvid_gc_trigger_log_length(),
            corvid_gc_total_ns(),
            corvid_gc_mark_count(),
            corvid_gc_sweep_count(),
            corvid_gc_cycle_reclaimed_count(),
            corvid_live_object_peak(),
        )
    }
}

fn runtime_delta_record(
    kind: &str,
    scenario: &str,
    trial_idx: usize,
    orchestration_ms: f64,
    target_releases: i64,
    threshold: Option<i64>,
    before: (i64, i64, i64, i64, i64, u64, u64, u64, u64, i64),
) -> Value {
    let (alloc_before, release_before, retain_before, release_calls_before, gc_trigger_before, gc_total_before, gc_mark_before, gc_sweep_before, gc_cycle_before, _) =
        before;
    let mut record = Map::new();
    unsafe {
        record.insert("kind".into(), Value::String(kind.into()));
        record.insert("scenario".into(), Value::String(scenario.into()));
        record.insert("stack".into(), Value::String("corvid".into()));
        record.insert("trial_idx".into(), Value::from(trial_idx as u64));
        record.insert("wall_ms".into(), Value::from(orchestration_ms));
        record.insert("external_wait_ms".into(), Value::from(0.0));
        record.insert("actual_external_wait_ms".into(), Value::from(0.0));
        record.insert("orchestration_ms".into(), Value::from(orchestration_ms));
        record.insert("target_release_scale".into(), Value::from(target_releases));
        record.insert(
            "gc_trigger_threshold".into(),
            threshold.map(Value::from).unwrap_or(Value::Null),
        );
        record.insert("allocs".into(), Value::from(corvid_alloc_count - alloc_before));
        record.insert("releases".into(), Value::from(corvid_release_count - release_before));
        record.insert(
            "rc_retain_count".into(),
            Value::from(corvid_retain_call_count - retain_before),
        );
        record.insert(
            "rc_release_count".into(),
            Value::from(corvid_release_call_count - release_calls_before),
        );
        record.insert(
            "gc_trigger_count".into(),
            Value::from(corvid_gc_trigger_log_length() - gc_trigger_before),
        );
        record.insert(
            "gc_total_ms".into(),
            Value::from((corvid_gc_total_ns() - gc_total_before) as f64 / 1_000_000.0),
        );
        record.insert(
            "gc_mark_count".into(),
            Value::from(corvid_gc_mark_count() - gc_mark_before),
        );
        record.insert(
            "gc_sweep_count".into(),
            Value::from(corvid_gc_sweep_count() - gc_sweep_before),
        );
        record.insert(
            "gc_cycle_count".into(),
            Value::from(corvid_gc_cycle_reclaimed_count() - gc_cycle_before),
        );
        record.insert(
            "peak_live_objects".into(),
            Value::from(corvid_live_object_peak()),
        );
    }
    Value::Object(record)
}

fn override_gc_fields(mut record: Value, gc_total_ms: f64, gc_trigger_count: i64) -> Value {
    if let Value::Object(map) = &mut record {
        map.insert("gc_total_ms".into(), Value::from(gc_total_ms));
        map.insert("gc_trigger_count".into(), Value::from(gc_trigger_count));
    }
    record
}

fn allocation_scaling_trial(target_releases: i64, trial_idx: usize) -> Value {
    let before = snapshot_runtime_counters();
    unsafe {
        corvid_gc_trigger_threshold = 0;
        corvid_reset_gc_alloc_counter();
    }
    let start = Instant::now();
    let left_bytes = b"corvid";
    let right_bytes = b"runtime";
    let gc_total_ms;
    unsafe {
        let left = corvid_string_from_bytes(left_bytes.as_ptr(), left_bytes.len() as i64);
        let right = corvid_string_from_bytes(right_bytes.as_ptr(), right_bytes.len() as i64);
        let mut roots = Vec::with_capacity(target_releases as usize + 2);
        roots.push(left);
        roots.push(right);
        for _ in 0..target_releases {
            roots.push(corvid_string_concat(left, right));
        }
        let gc_start = Instant::now();
        corvid_gc_from_roots(roots.as_mut_ptr(), roots.len());
        gc_total_ms = gc_start.elapsed().as_secs_f64() * 1000.0;
        for root in roots {
            corvid_release(root);
        }
    }
    override_gc_fields(
        runtime_delta_record(
        "allocation_scaling",
        &format!("allocation_scale_{target_releases}"),
        trial_idx,
        start.elapsed().as_secs_f64() * 1000.0,
        target_releases,
        None,
        before,
    ),
        gc_total_ms,
        1,
    )
}

fn trigger_sensitivity_trial(target_releases: i64, threshold: i64, trial_idx: usize) -> Value {
    let before = snapshot_runtime_counters();
    unsafe {
        corvid_gc_trigger_threshold = 0;
        corvid_reset_gc_alloc_counter();
    }
    let start = Instant::now();
    let mut gc_total_ms = 0.0;
    let mut gc_trigger_count = 0_i64;
    unsafe {
        let empty: &mut [*mut u8] = &mut [];
        for _ in 0..target_releases {
            let value = corvid_alloc_typed(8, &BOX_TYPEINFO);
            corvid_release(value);
            if threshold > 0 && (corvid_alloc_count - before.0) % threshold == 0 {
                let gc_start = Instant::now();
                corvid_gc_from_roots(empty.as_mut_ptr(), 0);
                gc_total_ms += gc_start.elapsed().as_secs_f64() * 1000.0;
                gc_trigger_count += 1;
            }
        }
    }
    override_gc_fields(
        runtime_delta_record(
        "gc_trigger_sensitivity",
        &format!("gc_trigger_{threshold}"),
        trial_idx,
        start.elapsed().as_secs_f64() * 1000.0,
        target_releases,
        Some(threshold),
        before,
    ),
        gc_total_ms,
        gc_trigger_count,
    )
}

fn run_allocation_scaling(trials: usize) -> Result<Vec<Value>> {
    let mut records = Vec::new();
    let only_scale = selected_release_scale()?;
    for &(target_releases, _) in ALLOCATION_SCALES {
        if only_scale.is_some() && only_scale != Some(target_releases) {
            continue;
        }
        for trial_idx in 1..=trials {
            records.push(allocation_scaling_trial(target_releases, trial_idx));
        }
    }
    Ok(records)
}

fn run_trigger_sensitivity(trials: usize) -> Result<Vec<Value>> {
    let mut records = Vec::new();
    let target_releases = selected_release_scale()?
        .or_else(|| ALLOCATION_SCALES.last().map(|(target, _)| *target))
        .context("highest allocation scale")?;
    let only_threshold = selected_gc_threshold()?;
    for &threshold in GC_THRESHOLDS {
        if only_threshold.is_some() && only_threshold != Some(threshold) {
            continue;
        }
        for trial_idx in 1..=trials {
            records.push(trigger_sensitivity_trial(target_releases, threshold, trial_idx));
        }
    }
    Ok(records)
}

fn ensure_cycle_profile_env() {
    static INIT: AtomicBool = AtomicBool::new(false);
    if INIT
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        std::env::set_var("CORVID_PROFILE_RUNTIME", "1");
    }
}

fn cycle_trial_record(cycle_pairs: usize, trial_idx: usize) -> Result<Value> {
    ensure_cycle_profile_env();
    let trial_start = Instant::now();
    let (alloc_before, release_before, retain_before, release_calls_before, _gc_trigger_before) =
        unsafe {
            (
                corvid_alloc_count,
                corvid_release_count,
                corvid_retain_call_count,
                corvid_release_call_count,
                corvid_gc_trigger_log_length(),
            )
        };
    let (_gc_total_before, gc_mark_before, gc_sweep_before, gc_cycle_before) = unsafe {
        corvid_reset_live_object_peak();
        (
            corvid_gc_total_ns(),
            corvid_gc_mark_count(),
            corvid_gc_sweep_count(),
            corvid_gc_cycle_reclaimed_count(),
        )
    };

    let mut nodes = Vec::with_capacity(cycle_pairs * 2);
    unsafe {
        for _ in 0..cycle_pairs * 2 {
            let node = corvid_alloc_typed(16, &CELL_TYPEINFO);
            let slots = node as *mut *mut u8;
            slots.write(std::ptr::null_mut());
            slots.add(1).write(std::ptr::null_mut());
            nodes.push(node);
        }
        for pair in 0..cycle_pairs {
            let a = nodes[pair * 2];
            let b = nodes[pair * 2 + 1];
            (a as *mut *mut u8).write(b);
            (a as *mut *mut u8).add(1).write(std::ptr::null_mut());
            (b as *mut *mut u8).write(a);
            (b as *mut *mut u8).add(1).write(std::ptr::null_mut());
        }
        let empty: &mut [*mut u8] = &mut [];
        let gc_start = Instant::now();
        corvid_gc_from_roots(empty.as_mut_ptr(), 0);
        let gc_total_ms = gc_start.elapsed().as_secs_f64() * 1000.0;
        let mut record = Map::new();
        record.insert("kind".into(), Value::String("cycle_stress".into()));
        record.insert(
            "scenario".into(),
            Value::String(format!("cycle_pairs_{cycle_pairs}")),
        );
        record.insert("stack".into(), Value::String("corvid".into()));
        record.insert("process_mode".into(), Value::String("runtime-ffi".into()));
        record.insert("trial_idx".into(), Value::from(trial_idx as u64));
        record.insert("cycle_pairs".into(), Value::from(cycle_pairs as u64));
        record.insert("wall_ms".into(), Value::from(trial_start.elapsed().as_secs_f64() * 1000.0));
        record.insert("external_wait_ms".into(), Value::from(0.0));
        record.insert("actual_external_wait_ms".into(), Value::from(0.0));
        record.insert(
            "orchestration_ms".into(),
            Value::from(trial_start.elapsed().as_secs_f64() * 1000.0),
        );
        record.insert("allocs".into(), Value::from(corvid_alloc_count - alloc_before));
        record.insert("releases".into(), Value::from(corvid_release_count - release_before));
        record.insert(
            "rc_retain_count".into(),
            Value::from(corvid_retain_call_count - retain_before),
        );
        record.insert(
            "rc_release_count".into(),
            Value::from(corvid_release_call_count - release_calls_before),
        );
        record.insert("gc_trigger_count".into(), Value::from(1));
        record.insert("gc_total_ms".into(), Value::from(gc_total_ms));
        record.insert(
            "gc_mark_count".into(),
            Value::from(corvid_gc_mark_count() - gc_mark_before),
        );
        record.insert(
            "gc_sweep_count".into(),
            Value::from(corvid_gc_sweep_count() - gc_sweep_before),
        );
        record.insert(
            "gc_cycle_count".into(),
            Value::from(corvid_gc_cycle_reclaimed_count() - gc_cycle_before),
        );
        record.insert(
            "peak_live_objects".into(),
            Value::from(corvid_live_object_peak()),
        );
        return Ok(Value::Object(record));
    }
}

fn run_cycle_stress(trials: usize) -> Result<Vec<Value>> {
    let mut records = Vec::new();
    let only_cycle_pairs = selected_cycle_pairs()?;
    for &cycle_pairs in CYCLE_SCALES {
        if only_cycle_pairs.is_some() && only_cycle_pairs != Some(cycle_pairs) {
            continue;
        }
        for trial_idx in 1..=trials {
            records.push(cycle_trial_record(cycle_pairs, trial_idx)?);
        }
    }
    Ok(records)
}
