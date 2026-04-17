use anyhow::{bail, Context, Result};
use corvid_driver::{build_or_get_cached_native, compile_to_ir};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::time::SystemTime;
use std::time::{Duration, Instant};

#[derive(Debug, Deserialize)]
struct Fixture {
    name: String,
    expected_replay_events: Vec<String>,
    steps: Vec<Step>,
}

#[derive(Debug, Deserialize)]
struct Step {
    name: String,
    kind: String,
    mock_response: Option<String>,
    mock_output: Option<Value>,
    external_latency_ms: u64,
}

#[derive(Default)]
struct TrialProfile {
    actual_external_wait_ms: f64,
    prompt_wait_actual_ms: f64,
    tool_wait_actual_ms: f64,
    allocs: Option<i64>,
    releases: Option<i64>,
    retain_calls: Option<i64>,
    release_calls: Option<i64>,
    gc_trigger_count: Option<i64>,
    safepoint_count: Option<i64>,
    stack_map_entry_count: Option<u64>,
    verify_drift_count: Option<i64>,
}

struct NativeServer {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    stderr: BufReader<ChildStderr>,
    compile_to_ir_ms: f64,
    cache_resolve_ms: f64,
    cache_hit: bool,
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("--server") => {
            let fixture_path = PathBuf::from(args.next().context("missing fixture path")?);
            let requests: usize = args
                .next()
                .context("missing request count")?
                .parse()
                .context("invalid request count")?;
            run_server_mode(&fixture_path, requests)
        }
        Some(first) => {
            let fixture_path = PathBuf::from(first);
            let trials: usize = args
                .next()
                .context("missing trials")?
                .parse()
                .context("invalid trials")?;
            let output_path = PathBuf::from(args.next().context("missing output path")?);
            run_batch_mode(&fixture_path, trials, &output_path)
        }
        None => bail!("usage: [--server <fixture.json> <requests>] | <fixture.json> <trials> <output.jsonl>"),
    }
}

fn run_batch_mode(fixture_path: &Path, trials: usize, output_path: &Path) -> Result<()> {
    let root = workspace_root()?;
    let fixture = load_fixture(fixture_path)?;
    let profile_enabled = std::env::var("CORVID_BENCH_PROFILE").ok().as_deref() == Some("1");
    let mut server = start_native_server(&root, &fixture, trials, profile_enabled)?;
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut out = String::new();
    for trial in 1..=trials {
        let record = run_trial(&fixture, &mut server, trial)?;
        out.push_str(&serde_json::to_string(&record)?);
        out.push('\n');
    }
    fs::write(output_path, out)?;
    finish_server(server)?;
    Ok(())
}

fn run_server_mode(fixture_path: &Path, requests: usize) -> Result<()> {
    let root = workspace_root()?;
    let fixture = load_fixture(fixture_path)?;
    let profile_enabled = std::env::var("CORVID_BENCH_PROFILE").ok().as_deref() == Some("1");
    let mut server = start_native_server(&root, &fixture, requests, profile_enabled)?;
    let stdin = std::io::stdin();
    let mut input = String::new();
    let mut locked = stdin.lock();
    loop {
        input.clear();
        if locked.read_line(&mut input)? == 0 {
            break;
        }
        let input = input.trim();
        if input.is_empty() {
            continue;
        }
        let request: Value = serde_json::from_str(input).context("parse server request")?;
        let trial = request
            .get("trial_idx")
            .and_then(Value::as_u64)
            .context("request trial_idx")? as usize;
        let record = run_trial(&fixture, &mut server, trial)?;
        println!("{}", serde_json::to_string(&record)?);
        std::io::stdout().flush()?;
    }
    finish_server(server)?;
    Ok(())
}

fn load_fixture(path: &Path) -> Result<Fixture> {
    serde_json::from_str(&fs::read_to_string(path)?).context("parse fixture")
}

fn workspace_root() -> Result<PathBuf> {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .map(Path::to_path_buf)
        .context("workspace root")
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

fn source_for_fixture(root: &Path, fixture: &str) -> PathBuf {
    root.join("benches").join("corvid").join("workloads").join(format!("{fixture}.cor"))
}

fn start_native_server(
    root: &Path,
    fixture: &Fixture,
    requests: usize,
    profile_enabled: bool,
) -> Result<NativeServer> {
    let tools_lib = build_tools_lib(root)?;
    let source_path = source_for_fixture(root, &fixture.name);
    let source = fs::read_to_string(&source_path)?;
    let compile_source = if profile_enabled {
        format!("{source}\n# benchmark-profiling\n")
    } else {
        source.clone()
    };

    let compile_start = Instant::now();
    let ir = compile_to_ir(&compile_source)
        .map_err(|diags| anyhow::anyhow!("compile diagnostics: {}", diags.len()))?;
    let compile_to_ir_ms = compile_start.elapsed().as_secs_f64() * 1000.0;
    let cache_start = Instant::now();
    let cached = build_or_get_cached_native(&source_path, &compile_source, &ir, Some(&tools_lib))?;
    let cache_resolve_ms = cache_start.elapsed().as_secs_f64() * 1000.0;
    let binary = cached.path;

    let launch_nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let trial_dir = root
        .join("benches")
        .join("corvid")
        .join("out")
        .join(&fixture.name)
        .join(format!("persistent-{}-{launch_nonce}", std::process::id()));
    fs::create_dir_all(trial_dir.join("target").join("trace"))?;

    let prompt_replies = prompt_replies_repeated(fixture, requests);
    let prompt_latencies = prompt_latencies_repeated(fixture, requests);
    let tool_replies = tool_replies_repeated(fixture, requests);
    let tool_latencies = tool_latencies_repeated(fixture, requests);

    let mut command = Command::new(&binary);
    command
        .current_dir(&trial_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("CORVID_MODEL", "mock-bench")
        .env("CORVID_APPROVE_AUTO", "1")
        .env("CORVID_TEST_MOCK_LLM", "1")
        .env("CORVID_TEST_MOCK_LLM_REPLIES", serde_json::to_string(&prompt_replies)?)
        .env("CORVID_TEST_MOCK_LLM_LATENCY_MS", serde_json::to_string(&prompt_latencies)?)
        .env("CORVID_BENCH_TOOL_RESPONSES", serde_json::to_string(&tool_replies)?)
        .env("CORVID_BENCH_TOOL_LATENCIES_MS", serde_json::to_string(&tool_latencies)?)
        .env("CORVID_BENCH_SERVER", "1")
        .env("CORVID_PROFILE_EVENTS", "1")
        .env("CORVID_BENCH_COMPILE_TO_IR_MS", format!("{compile_to_ir_ms:.6}"))
        .env("CORVID_BENCH_CACHE_RESOLVE_MS", format!("{cache_resolve_ms:.6}"))
        .env("CORVID_BENCH_CACHE_HIT", if cached.from_cache { "1" } else { "0" });
    if profile_enabled {
        command.env("CORVID_PROFILE_RUNTIME", "1");
    }

    let mut child = command
        .spawn()
        .with_context(|| format!("spawn persistent `{}`", binary.display()))?;
    let stdin = child.stdin.take().context("persistent stdin")?;
    let stdout = BufReader::new(child.stdout.take().context("persistent stdout")?);
    let stderr = BufReader::new(child.stderr.take().context("persistent stderr")?);
    Ok(NativeServer {
        child,
        stdin,
        stdout,
        stderr,
        compile_to_ir_ms,
        cache_resolve_ms,
        cache_hit: cached.from_cache,
    })
}

fn finish_server(mut server: NativeServer) -> Result<()> {
    drop(server.stdin);
    let status = server.child.wait()?;
    if !status.success() {
        bail!("persistent native benchmark exited with {status}");
    }
    Ok(())
}

fn run_trial(fixture: &Fixture, server: &mut NativeServer, trial: usize) -> Result<Value> {
    let expected_stdout = expected_stdout(fixture)?;
    let retry_sleep_ms: u64 = fixture
        .steps
        .iter()
        .filter(|s| s.kind == "retry_sleep")
        .map(|s| s.external_latency_ms)
        .sum();
    let external_wait_ms: u64 = fixture.steps.iter().map(|s| s.external_latency_ms).sum();
    let logical_steps_recorded = fixture.steps.len();
    let bytes_per_step = 0.0;
    let trace_size_raw_bytes = 0u64;

    let start = Instant::now();
    writeln!(server.stdin, "{trial}")?;
    server.stdin.flush()?;
    let stdout = read_stdout_line(&mut server.stdout)?;
    let mut profile = read_trial_profile(&mut server.stderr)?;

    let retry_sleep_start = Instant::now();
    if retry_sleep_ms > 0 {
        std::thread::sleep(Duration::from_millis(retry_sleep_ms));
    }
    let retry_sleep_actual_ms = retry_sleep_start.elapsed().as_secs_f64() * 1000.0;
    profile.actual_external_wait_ms += retry_sleep_actual_ms;
    let total_wall_ms = start.elapsed().as_secs_f64() * 1000.0;

    Ok(json!({
        "implementation": "corvid-native",
        "process_mode": "persistent",
        "fixture": fixture.name,
        "trial": trial,
        "success": true,
        "stdout_match": stdout == expected_stdout,
        "total_wall_ms": total_wall_ms,
        "external_wait_ms": external_wait_ms,
        "actual_external_wait_ms": profile.actual_external_wait_ms,
        "external_wait_bias_ms": profile.actual_external_wait_ms - external_wait_ms as f64,
        "orchestration_overhead_ms": total_wall_ms - profile.actual_external_wait_ms,
        "runner_total_wall_ms": total_wall_ms,
        "compile_to_ir_ms": server.compile_to_ir_ms,
        "cache_resolve_ms": server.cache_resolve_ms,
        "binary_exec_ms": total_wall_ms - retry_sleep_actual_ms,
        "retry_sleep_nominal_ms": retry_sleep_ms,
        "retry_sleep_actual_ms": retry_sleep_actual_ms,
        "cache_hit": server.cache_hit,
        "trace_size_raw_bytes": trace_size_raw_bytes,
        "logical_steps_recorded": logical_steps_recorded,
        "bytes_per_step": bytes_per_step,
        "replay_supported": false,
        "expected_replay_steps": fixture.expected_replay_events.len(),
        "prompt_wait_nominal_ms": fixture.steps.iter().filter(|s| s.kind == "prompt").map(|s| s.external_latency_ms).sum::<u64>(),
        "prompt_wait_actual_ms": profile.prompt_wait_actual_ms,
        "tool_wait_nominal_ms": fixture.steps.iter().filter(|s| s.kind == "tool").map(|s| s.external_latency_ms).sum::<u64>(),
        "tool_wait_actual_ms": profile.tool_wait_actual_ms,
        "allocs": profile.allocs,
        "releases": profile.releases,
        "retain_calls": profile.retain_calls,
        "release_calls": profile.release_calls,
        "gc_trigger_count": profile.gc_trigger_count,
        "safepoint_count": profile.safepoint_count,
        "stack_map_entry_count": profile.stack_map_entry_count,
        "verify_drift_count": profile.verify_drift_count,
    }))
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
            bail!("persistent native benchmark ended before emitting trial profile");
        }
        if let Some(raw) = line.trim().strip_prefix("CORVID_PROFILE_JSON=") {
            let value: Value = serde_json::from_str(raw).context("parse wait profile")?;
            if value.get("kind").and_then(Value::as_str) == Some("wait") {
                let actual_ms = value
                    .get("actual_ms")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                profile.actual_external_wait_ms += actual_ms;
                match value.get("source_kind").and_then(Value::as_str) {
                    Some("prompt") => profile.prompt_wait_actual_ms += actual_ms,
                    Some("tool") => profile.tool_wait_actual_ms += actual_ms,
                    _ => {}
                }
            }
            continue;
        }
        if let Some(raw) = line.trim().strip_prefix("CORVID_BENCH_TRIAL=") {
            let value: Value = serde_json::from_str(raw).context("parse benchmark trial profile")?;
            profile.allocs = value.get("allocs").and_then(Value::as_i64);
            profile.releases = value.get("releases").and_then(Value::as_i64);
            profile.retain_calls = value.get("retain_calls").and_then(Value::as_i64);
            profile.release_calls = value.get("release_calls").and_then(Value::as_i64);
            profile.gc_trigger_count = value.get("gc_trigger_count").and_then(Value::as_i64);
            profile.safepoint_count = value.get("safepoint_count").and_then(Value::as_i64);
            profile.stack_map_entry_count =
                value.get("stack_map_entry_count").and_then(Value::as_u64);
            profile.verify_drift_count =
                value.get("verify_drift_count").and_then(Value::as_i64);
            return Ok(profile);
        }
    }
}

fn prompt_replies_repeated(fixture: &Fixture, repeats: usize) -> BTreeMap<String, Value> {
    fixture
        .steps
        .iter()
        .filter(|s| s.kind == "prompt")
        .map(|s| {
            let values = (0..repeats)
                .map(|_| Value::String(s.mock_response.clone().unwrap_or_default()))
                .collect::<Vec<_>>();
            (s.name.clone(), Value::Array(values))
        })
        .collect()
}

fn prompt_latencies_repeated(fixture: &Fixture, repeats: usize) -> BTreeMap<String, Value> {
    fixture
        .steps
        .iter()
        .filter(|s| s.kind == "prompt")
        .map(|s| {
            let values = (0..repeats)
                .map(|_| Value::from(s.external_latency_ms))
                .collect::<Vec<_>>();
            (s.name.clone(), Value::Array(values))
        })
        .collect()
}

fn tool_replies_repeated(fixture: &Fixture, repeats: usize) -> BTreeMap<String, Value> {
    let mut out: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for _ in 0..repeats {
        for step in fixture.steps.iter().filter(|s| s.kind == "tool") {
            let base = tool_base_name(&step.name);
            out.entry(base)
                .or_default()
                .push(Value::String(step.mock_output.clone().unwrap_or(Value::Null).to_string()));
        }
    }
    out.into_iter().map(|(k, v)| (k, Value::Array(v))).collect()
}

fn tool_latencies_repeated(fixture: &Fixture, repeats: usize) -> BTreeMap<String, Value> {
    let mut out: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for _ in 0..repeats {
        for step in fixture.steps.iter().filter(|s| s.kind == "tool") {
            let base = tool_base_name(&step.name);
            out.entry(base)
                .or_default()
                .push(Value::from(step.external_latency_ms));
        }
    }
    out.into_iter().map(|(k, v)| (k, Value::Array(v))).collect()
}

fn tool_base_name(name: &str) -> String {
    name.split("_attempt_").next().unwrap_or(name).to_string()
}

fn expected_stdout(fixture: &Fixture) -> Result<String> {
    match fixture.name.as_str() {
        "baseline_control" => Ok("1".to_string()),
        "tool_loop" => Ok(fixture_tool_output(fixture, "fetch_open_orders")?),
        "retry_workflow" => Ok(fixture_tool_output(fixture, "fetch_shipment_status_attempt_3")?),
        "approval_workflow" => Ok(fixture_tool_output(fixture, "issue_refund")?),
        "replay_trace" => fixture
            .steps
            .iter()
            .find(|s| s.name == "draft_reply")
            .and_then(|s| s.mock_response.clone())
            .context("draft_reply mock_response"),
        other => bail!("unknown fixture `{other}`"),
    }
}

fn fixture_tool_output(fixture: &Fixture, step_name: &str) -> Result<String> {
    fixture
        .steps
        .iter()
        .find(|s| s.name == step_name)
        .and_then(|s| s.mock_output.clone())
        .map(|v| v.to_string())
        .context("tool output")
}
