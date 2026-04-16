use anyhow::{bail, Context, Result};
use corvid_driver::{build_or_get_cached_native, compile_to_ir};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
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
struct RuntimeProfile {
    actual_external_wait_ms: f64,
    prompt_wait_nominal_ms: u64,
    prompt_wait_actual_ms: f64,
    tool_wait_nominal_ms: u64,
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

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let fixture_path = PathBuf::from(args.next().context("usage: <fixture.json> <trials> <output.jsonl>")?);
    let trials: usize = args
        .next()
        .context("missing trials")?
        .parse()
        .context("invalid trials")?;
    let output_path = PathBuf::from(args.next().context("missing output path")?);

    let root = workspace_root()?;
    let fixture: Fixture = serde_json::from_str(&fs::read_to_string(&fixture_path)?)?;
    let tools_lib = build_tools_lib(&root)?;
    let source_path = source_for_fixture(&root, &fixture.name);
    let source = fs::read_to_string(&source_path)?;
    let profile_enabled = std::env::var("CORVID_BENCH_PROFILE").ok().as_deref() == Some("1");
    let compile_source = if profile_enabled {
        format!("{source}\n# benchmark-profiling\n")
    } else {
        source.clone()
    };

    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut out = String::new();
    for trial in 1..=trials {
        let runner_start = Instant::now();
        let compile_start = Instant::now();
        let ir = compile_to_ir(&compile_source)
            .map_err(|diags| anyhow::anyhow!("compile diagnostics: {}", diags.len()))?;
        let compile_to_ir_ms = compile_start.elapsed().as_secs_f64() * 1000.0;

        let cache_start = Instant::now();
        let cached =
            build_or_get_cached_native(&source_path, &compile_source, &ir, Some(&tools_lib))?;
        let cache_resolve_ms = cache_start.elapsed().as_secs_f64() * 1000.0;
        let binary = cached.path;

        let trial_dir = root
            .join("benches")
            .join("corvid")
            .join("out")
            .join(&fixture.name)
            .join(format!("trial-{trial}"));
        if trial_dir.exists() {
            fs::remove_dir_all(&trial_dir)?;
        }
        fs::create_dir_all(trial_dir.join("target").join("trace"))?;

        let prompt_replies = prompt_replies(&fixture);
        let prompt_latencies = prompt_latencies(&fixture);
        let tool_replies = tool_replies(&fixture);
        let tool_latencies = tool_latencies(&fixture);
        let retry_sleep_ms: u64 = fixture
            .steps
            .iter()
            .filter(|s| s.kind == "retry_sleep")
            .map(|s| s.external_latency_ms)
            .sum();
        let external_wait_ms: u64 = fixture.steps.iter().map(|s| s.external_latency_ms).sum();

        let exec_start = Instant::now();
        let mut command = Command::new(&binary);
        command
            .current_dir(&trial_dir)
            .env("CORVID_MODEL", "mock-bench")
            .env("CORVID_APPROVE_AUTO", "1")
            .env("CORVID_TEST_MOCK_LLM", "1")
            .env("CORVID_TEST_MOCK_LLM_REPLIES", serde_json::to_string(&prompt_replies)?)
            .env("CORVID_TEST_MOCK_LLM_LATENCY_MS", serde_json::to_string(&prompt_latencies)?)
            .env("CORVID_BENCH_TOOL_RESPONSES", serde_json::to_string(&tool_replies)?)
            .env("CORVID_BENCH_TOOL_LATENCIES_MS", serde_json::to_string(&tool_latencies)?);
        if profile_enabled {
            command
                .env("CORVID_PROFILE_EVENTS", "1")
                .env("CORVID_PROFILE_RUNTIME", "1");
        }

        let status = command
            .output()
            .with_context(|| format!("spawn `{}`", binary.display()))?;
        let binary_exec_ms = exec_start.elapsed().as_secs_f64() * 1000.0;

        let retry_sleep_start = Instant::now();
        if retry_sleep_ms > 0 {
            std::thread::sleep(Duration::from_millis(retry_sleep_ms));
        }
        let retry_sleep_actual_ms = retry_sleep_start.elapsed().as_secs_f64() * 1000.0;
        let total_wall_ms = binary_exec_ms + retry_sleep_actual_ms;
        let runner_total_wall_ms = runner_start.elapsed().as_secs_f64() * 1000.0;

        let stdout = String::from_utf8_lossy(&status.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&status.stderr).into_owned();
        let expected_stdout = expected_stdout(&fixture)?;
        let trace_dir = trial_dir.join("target").join("trace");
        let (trace_size_raw_bytes, logical_steps_recorded) = trace_stats(&trace_dir)?;
        let bytes_per_step = if logical_steps_recorded > 0 {
            trace_size_raw_bytes as f64 / logical_steps_recorded as f64
        } else {
            0.0
        };
        let runtime_profile = parse_runtime_profile(&stderr);
        let actual_external_wait_ms =
            runtime_profile.actual_external_wait_ms + retry_sleep_actual_ms;

        let record = json!({
            "implementation": "corvid-native",
            "fixture": fixture.name,
            "trial": trial,
            "success": status.status.success(),
            "stdout_match": stdout == expected_stdout,
            "total_wall_ms": total_wall_ms,
            "external_wait_ms": external_wait_ms,
            "actual_external_wait_ms": actual_external_wait_ms,
            "external_wait_bias_ms": actual_external_wait_ms - external_wait_ms as f64,
            "orchestration_overhead_ms": total_wall_ms - external_wait_ms as f64,
            "runner_total_wall_ms": runner_total_wall_ms,
            "compile_to_ir_ms": compile_to_ir_ms,
            "cache_resolve_ms": cache_resolve_ms,
            "binary_exec_ms": binary_exec_ms,
            "retry_sleep_nominal_ms": retry_sleep_ms,
            "retry_sleep_actual_ms": retry_sleep_actual_ms,
            "cache_hit": cached.from_cache,
            "trace_size_raw_bytes": trace_size_raw_bytes,
            "logical_steps_recorded": logical_steps_recorded,
            "bytes_per_step": bytes_per_step,
            "replay_supported": false,
            "expected_replay_steps": fixture.expected_replay_events.len(),
            "prompt_wait_nominal_ms": runtime_profile.prompt_wait_nominal_ms,
            "prompt_wait_actual_ms": runtime_profile.prompt_wait_actual_ms,
            "tool_wait_nominal_ms": runtime_profile.tool_wait_nominal_ms,
            "tool_wait_actual_ms": runtime_profile.tool_wait_actual_ms,
            "allocs": runtime_profile.allocs,
            "releases": runtime_profile.releases,
            "retain_calls": runtime_profile.retain_calls,
            "release_calls": runtime_profile.release_calls,
            "gc_trigger_count": runtime_profile.gc_trigger_count,
            "safepoint_count": runtime_profile.safepoint_count,
            "stack_map_entry_count": runtime_profile.stack_map_entry_count,
            "verify_drift_count": runtime_profile.verify_drift_count,
        });
        out.push_str(&serde_json::to_string(&record)?);
        out.push('\n');
    }
    fs::write(output_path, out)?;
    Ok(())
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
    if built.exists() {
        return Ok(built);
    }
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

fn prompt_replies(fixture: &Fixture) -> BTreeMap<String, Value> {
    fixture
        .steps
        .iter()
        .filter(|s| s.kind == "prompt")
        .map(|s| {
            (
                s.name.clone(),
                Value::String(s.mock_response.clone().unwrap_or_default()),
            )
        })
        .collect()
}

fn prompt_latencies(fixture: &Fixture) -> BTreeMap<String, u64> {
    fixture
        .steps
        .iter()
        .filter(|s| s.kind == "prompt")
        .map(|s| (s.name.clone(), s.external_latency_ms))
        .collect()
}

fn tool_replies(fixture: &Fixture) -> BTreeMap<String, Value> {
    let mut out: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for step in fixture.steps.iter().filter(|s| s.kind == "tool") {
        let base = tool_base_name(&step.name);
        out.entry(base)
            .or_default()
            .push(Value::String(step.mock_output.clone().unwrap_or(Value::Null).to_string()));
    }
    out.into_iter().map(|(k, v)| (k, Value::Array(v))).collect()
}

fn tool_latencies(fixture: &Fixture) -> BTreeMap<String, Value> {
    let mut out: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for step in fixture.steps.iter().filter(|s| s.kind == "tool") {
        let base = tool_base_name(&step.name);
        out.entry(base)
            .or_default()
            .push(Value::from(step.external_latency_ms));
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

fn trace_stats(trace_dir: &Path) -> Result<(u64, usize)> {
    if !trace_dir.exists() {
        return Ok((0, 0));
    }
    let mut bytes = 0u64;
    let mut lines = 0usize;
    for entry in fs::read_dir(trace_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            let data = fs::read(entry.path())?;
            bytes += data.len() as u64;
            lines += String::from_utf8_lossy(&data).lines().count();
        }
    }
    Ok((bytes, lines))
}

fn parse_runtime_profile(stderr: &str) -> RuntimeProfile {
    let mut profile = RuntimeProfile::default();
    for line in stderr.lines() {
        if let Some(raw) = line.strip_prefix("CORVID_PROFILE_JSON=") {
            if let Ok(value) = serde_json::from_str::<Value>(raw) {
                if value.get("kind").and_then(Value::as_str) == Some("wait") {
                    let actual_ms = value
                        .get("actual_ms")
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0);
                    let nominal_ms = value
                        .get("nominal_ms")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    profile.actual_external_wait_ms += actual_ms;
                    match value.get("source_kind").and_then(Value::as_str) {
                        Some("prompt") => {
                            profile.prompt_wait_nominal_ms += nominal_ms;
                            profile.prompt_wait_actual_ms += actual_ms;
                        }
                        Some("tool") => {
                            profile.tool_wait_nominal_ms += nominal_ms;
                            profile.tool_wait_actual_ms += actual_ms;
                        }
                        _ => {}
                    }
                }
            }
        } else if let Some(raw) = line.strip_prefix("CORVID_PROFILE_RUNTIME=") {
            if let Ok(value) = serde_json::from_str::<Value>(raw) {
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
            }
        }
    }
    profile
}
