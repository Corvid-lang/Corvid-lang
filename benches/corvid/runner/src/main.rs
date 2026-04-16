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
    let ir = compile_to_ir(&source).map_err(|diags| anyhow::anyhow!("compile diagnostics: {}", diags.len()))?;
    let binary = build_or_get_cached_native(&source_path, &source, &ir, Some(&tools_lib))?.path;

    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut out = String::new();
    for trial in 1..=trials {
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

        let start = Instant::now();
        let status = Command::new(&binary)
            .current_dir(&trial_dir)
            .env("CORVID_MODEL", "mock-bench")
            .env("CORVID_APPROVE_AUTO", "1")
            .env("CORVID_TEST_MOCK_LLM", "1")
            .env("CORVID_TEST_MOCK_LLM_REPLIES", serde_json::to_string(&prompt_replies)?)
            .env("CORVID_TEST_MOCK_LLM_LATENCY_MS", serde_json::to_string(&prompt_latencies)?)
            .env("CORVID_BENCH_TOOL_RESPONSES", serde_json::to_string(&tool_replies)?)
            .env("CORVID_BENCH_TOOL_LATENCIES_MS", serde_json::to_string(&tool_latencies)?)
            .output()
            .with_context(|| format!("spawn `{}`", binary.display()))?;
        if retry_sleep_ms > 0 {
            std::thread::sleep(Duration::from_millis(retry_sleep_ms));
        }
        let elapsed = start.elapsed();

        let stdout = String::from_utf8_lossy(&status.stdout).trim().to_string();
        let expected_stdout = expected_stdout(&fixture)?;
        let trace_dir = trial_dir.join("target").join("trace");
        let (trace_size_raw_bytes, logical_steps_recorded) = trace_stats(&trace_dir)?;
        let bytes_per_step = if logical_steps_recorded > 0 {
            trace_size_raw_bytes as f64 / logical_steps_recorded as f64
        } else {
            0.0
        };

        let record = json!({
            "implementation": "corvid-native",
            "fixture": fixture.name,
            "trial": trial,
            "success": status.status.success(),
            "stdout_match": stdout == expected_stdout,
            "total_wall_ms": elapsed.as_secs_f64() * 1000.0,
            "external_wait_ms": external_wait_ms,
            "orchestration_overhead_ms": (elapsed.as_secs_f64() * 1000.0) - external_wait_ms as f64,
            "trace_size_raw_bytes": trace_size_raw_bytes,
            "logical_steps_recorded": logical_steps_recorded,
            "bytes_per_step": bytes_per_step,
            "replay_supported": false,
            "expected_replay_steps": fixture.expected_replay_events.len(),
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
