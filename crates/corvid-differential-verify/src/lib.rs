use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use anyhow::{anyhow, bail, Context, Result};
use corvid_ast::{BackpressurePolicy, Decl, DimensionValue, PromptDecl, ToolDecl, TypeRef};
use corvid_codegen_cl::build_native_to_disk;
use corvid_driver::{
    compile_to_ir, run_ir_with_runtime, MockAdapter, ProgrammaticApprover, Runtime, Tracer,
};
use corvid_resolve::resolve;
use corvid_syntax::{lex, parse_file};
use corvid_trace_schema::TraceEvent;
use corvid_types::{analyze_effects, typecheck, EffectRegistry};
use serde::{Deserialize, Serialize};

mod diff;
pub mod fuzz;
pub mod render;
pub mod rewrite;
mod shrink;

use diff::diff_reports;
pub use render::{render_corpus_grid, render_report};
pub use shrink::shrink_program;

const VERIFY_MODEL: &str = "verify-mock";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Tier {
    Checker,
    Interpreter,
    Native,
    Replay,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum TrustLevel {
    Autonomous,
    SupervisorRequired,
    HumanRequired,
    Custom(String),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DataCategory(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LatencyLevel {
    Instant,
    Fast,
    Medium,
    Slow,
    Streaming { backpressure: BackpressurePolicy },
    Custom(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectProfile {
    pub cost: f64,
    pub trust: TrustLevel,
    pub reversible: bool,
    pub data: BTreeSet<DataCategory>,
    pub latency: LatencyLevel,
    pub confidence: f64,
    pub extra: BTreeMap<String, DimensionValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlameAttribution {
    pub tier: Tier,
    pub file: PathBuf,
    pub commit: String,
    pub author: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DivergenceClass {
    StaticOverapproximated,
    StaticTooLoose,
    TierMismatch,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Divergence {
    pub dimension: String,
    pub classification: DivergenceClass,
    pub values: BTreeMap<Tier, serde_json::Value>,
    pub attribution: Vec<BlameAttribution>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TierReport {
    pub tier: Tier,
    pub profile: EffectProfile,
    pub effect_names: Vec<String>,
    pub trace_path: Option<PathBuf>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DivergenceReport {
    pub program: PathBuf,
    pub reports: [TierReport; 4],
    pub divergences: Vec<Divergence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShrinkResult {
    pub original: PathBuf,
    pub output: PathBuf,
    pub removed_lines: usize,
}

struct Frontend {
    path: PathBuf,
    file: corvid_ast::File,
    resolved: corvid_resolve::Resolved,
    registry: EffectRegistry,
    ir: corvid_ir::IrFile,
    entry_agent: String,
    prompts: HashMap<String, PromptDecl>,
    tools: HashMap<String, ToolDecl>,
}

pub fn verify_program(path: &Path) -> Result<DivergenceReport> {
    let frontend = Frontend::load(path)?;
    let checker = checker_report(&frontend)?;
    let interpreter = interpreter_report(&frontend)?;
    let native = native_report(&frontend)?;
    let replay = replay_report(&frontend, interpreter.trace_path.as_deref())?;
    let reports = [checker, interpreter, native, replay];
    let divergences = diff_reports(&reports)?;
    Ok(DivergenceReport {
        program: path.to_path_buf(),
        reports,
        divergences,
    })
}

pub fn verify_corpus(dir: &Path) -> Result<Vec<DivergenceReport>> {
    let mut files = collect_programs(dir)?;
    files.sort();
    files
        .into_iter()
        .map(|path| verify_program(&path))
        .collect()
}

impl Frontend {
    fn load(path: &Path) -> Result<Self> {
        let source = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read `{}`", path.display()))?;
        let tokens = lex(&source).map_err(|errs| anyhow!("lex failed: {errs:?}"))?;
        let (file, parse_errors) = parse_file(&tokens);
        if !parse_errors.is_empty() {
            bail!("parse failed for `{}`: {:?}", path.display(), parse_errors);
        }
        let resolved = resolve(&file);
        if !resolved.errors.is_empty() {
            bail!(
                "resolve failed for `{}`: {:?}",
                path.display(),
                resolved.errors
            );
        }
        let checked = typecheck(&file, &resolved);
        if !checked.errors.is_empty() {
            bail!(
                "typecheck failed for `{}`: {:?}",
                path.display(),
                checked.errors
            );
        }
        let ir = compile_to_ir(&source)
            .map_err(|diagnostics| anyhow!("IR lowering failed: {:?}", diagnostics))?;
        let entry_agent = select_entry_agent(&ir)?;
        let effect_decls: Vec<_> = file
            .decls
            .iter()
            .filter_map(|decl| match decl {
                Decl::Effect(effect) => Some(effect.clone()),
                _ => None,
            })
            .collect();
        let registry = EffectRegistry::from_decls(&effect_decls);
        let prompts = file
            .decls
            .iter()
            .filter_map(|decl| match decl {
                Decl::Prompt(prompt) => Some((prompt.name.name.clone(), prompt.clone())),
                _ => None,
            })
            .collect();
        let tools = file
            .decls
            .iter()
            .filter_map(|decl| match decl {
                Decl::Tool(tool) => Some((tool.name.name.clone(), tool.clone())),
                _ => None,
            })
            .collect();
        Ok(Self {
            path: path.to_path_buf(),
            file,
            resolved,
            registry,
            ir,
            entry_agent,
            prompts,
            tools,
        })
    }
}

fn checker_report(frontend: &Frontend) -> Result<TierReport> {
    let summaries = analyze_effects(&frontend.file, &frontend.resolved, &frontend.registry);
    let summary = summaries
        .into_iter()
        .find(|summary| summary.agent_name == frontend.entry_agent)
        .ok_or_else(|| anyhow!("missing effect summary for `{}`", frontend.entry_agent))?;
    Ok(TierReport {
        tier: Tier::Checker,
        profile: normalize_profile(&summary.composed.dimensions),
        effect_names: summary.composed.effect_names,
        trace_path: None,
        notes: vec!["static all-path type-checker profile".into()],
    })
}

fn interpreter_report(frontend: &Frontend) -> Result<TierReport> {
    let trace_root = persistent_verify_dir("interpreter")?;
    let tracer = Tracer::open(&trace_root, "verify-interpreter");
    let trace_path = tracer.path().to_path_buf();
    {
        let runtime = interpreter_runtime(frontend, tracer)?;
        let tokio = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("failed to create interpreter tokio runtime")?;
        tokio
            .block_on(async { run_ir_with_runtime(&frontend.ir, None, vec![], &runtime).await })
            .with_context(|| format!("interpreter run failed for `{}`", frontend.path.display()))?;
    }

    let events = load_trace_events(&trace_path)?;
    let (profile, effect_names) = profile_from_trace(frontend, &events)?;
    Ok(TierReport {
        tier: Tier::Interpreter,
        profile,
        effect_names,
        trace_path: Some(trace_path),
        notes: vec!["dynamic profile reconstructed from interpreter trace".into()],
    })
}

fn native_report(frontend: &Frontend) -> Result<TierReport> {
    ensure_native_runtime_staticlib()?;
    let run_root = persistent_verify_dir("native")?;
    let bin_path = run_root.join("verify_program");
    let trace_path = run_root.join("native-trace.jsonl");
    let binary =
        build_native_to_disk(&frontend.ir, "corvid_verify", &bin_path, &[]).map_err(|err| {
            anyhow!(
                "native build failed for `{}`: {err}",
                frontend.path.display()
            )
        })?;
    let replies = serde_json::to_string(&mock_reply_map(frontend)?)
        .context("failed to serialize native mock replies")?;
    let output = Command::new(&binary)
        .current_dir(&run_root)
        .env("CORVID_APPROVE_AUTO", "1")
        .env("CORVID_TEST_MOCK_LLM", "1")
        .env("CORVID_TEST_MOCK_LLM_REPLIES", replies)
        .env("CORVID_MODEL", VERIFY_MODEL)
        .env("CORVID_TRACE_PATH", &trace_path)
        .output()
        .with_context(|| format!("failed to run native binary `{}`", binary.display()))?;
    let events = load_trace_events(&trace_path).with_context(|| {
        format!(
            "native run for `{}` did not produce a readable trace at `{}`",
            frontend.path.display(),
            trace_path.display()
        )
    })?;
    let (profile, effect_names) = profile_from_trace(frontend, &events)?;
    let mut notes = vec![
        "dynamic profile reconstructed from native runtime trace".into(),
        format!(
            "executed native binary, read trace from CORVID_TRACE_PATH={}",
            trace_path.display()
        ),
    ];
    if !output.status.success() {
        notes.push(format!(
            "native binary exited non-zero (status={}): stdout=`{}` stderr=`{}`",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(TierReport {
        tier: Tier::Native,
        profile,
        effect_names,
        trace_path: Some(trace_path),
        notes,
    })
}

fn replay_report(frontend: &Frontend, trace_path: Option<&Path>) -> Result<TierReport> {
    let trace_path = trace_path.ok_or_else(|| anyhow!("replay tier requires a trace file"))?;
    let events = load_trace_events(trace_path)?;
    let (profile, effect_names) = profile_from_trace(frontend, &events)?;
    Ok(TierReport {
        tier: Tier::Replay,
        profile,
        effect_names,
        trace_path: Some(trace_path.to_path_buf()),
        notes: vec!["profile reconstructed by replaying persisted JSONL trace".into()],
    })
}

fn interpreter_runtime(frontend: &Frontend, tracer: Tracer) -> Result<Runtime> {
    let mock = MockAdapter::new(VERIFY_MODEL);
    for (prompt_name, reply) in mock_reply_map(frontend)? {
        mock.add_reply(prompt_name, reply);
    }
    Ok(Runtime::builder()
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .llm(Arc::new(mock))
        .default_model(VERIFY_MODEL)
        .tracer(tracer)
        .build())
}

fn mock_reply_map(frontend: &Frontend) -> Result<BTreeMap<String, serde_json::Value>> {
    frontend
        .prompts
        .values()
        .map(|prompt| {
            Ok((
                prompt.name.name.clone(),
                mock_value_for_type(&prompt.return_ty).with_context(|| {
                    format!(
                        "unsupported prompt return type in `{}`",
                        frontend.path.display()
                    )
                })?,
            ))
        })
        .collect()
}

fn mock_value_for_type(ty: &TypeRef) -> Result<serde_json::Value> {
    match ty {
        TypeRef::Named { name, .. } => match name.name.as_str() {
            "Int" => Ok(serde_json::json!(7)),
            "Float" => Ok(serde_json::json!(3.14)),
            "Bool" => Ok(serde_json::json!(true)),
            "String" => Ok(serde_json::json!("mock")),
            "Nothing" => Ok(serde_json::Value::Null),
            other => bail!("no verifier mock value for prompt return type `{other}`"),
        },
        TypeRef::Generic { name, args, .. } if name.name == "Grounded" && args.len() == 1 => {
            mock_value_for_type(&args[0])
        }
        _ => bail!("unsupported prompt return shape `{ty:?}`"),
    }
}

fn profile_from_trace(
    frontend: &Frontend,
    events: &[TraceEvent],
) -> Result<(EffectProfile, Vec<String>)> {
    let mut effect_names = Vec::new();
    for event in events {
        match event {
            TraceEvent::LlmCall { prompt, .. } => {
                let prompt_decl = frontend
                    .prompts
                    .get(prompt)
                    .ok_or_else(|| anyhow!("trace referenced unknown prompt `{prompt}`"))?;
                for effect in &prompt_decl.effect_row.effects {
                    effect_names.push(effect.name.name.clone());
                }
            }
            TraceEvent::ToolCall { tool, .. } => {
                let tool_decl = frontend
                    .tools
                    .get(tool)
                    .ok_or_else(|| anyhow!("trace referenced unknown tool `{tool}`"))?;
                for effect in &tool_decl.effect_row.effects {
                    effect_names.push(effect.name.name.clone());
                }
                if matches!(tool_decl.effect, corvid_ast::Effect::Dangerous) {
                    effect_names.push("dangerous".into());
                }
            }
            _ => {}
        }
    }
    effect_names.sort();
    effect_names.dedup();
    let refs: Vec<_> = effect_names.iter().map(|name| name.as_str()).collect();
    let composed = frontend.registry.compose(&refs);
    Ok((normalize_profile(&composed.dimensions), effect_names))
}

fn normalize_profile(dimensions: &HashMap<String, DimensionValue>) -> EffectProfile {
    let extra = dimensions
        .iter()
        .filter(|(name, _)| !is_builtin_dimension(name))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    EffectProfile {
        cost: match dimensions.get("cost") {
            Some(DimensionValue::Cost(value)) => *value,
            _ => 0.0,
        },
        trust: normalize_trust(dimensions.get("trust")),
        reversible: match dimensions.get("reversible") {
            Some(DimensionValue::Bool(value)) => *value,
            _ => true,
        },
        data: normalize_data(dimensions.get("data")),
        latency: normalize_latency(dimensions.get("latency")),
        confidence: match dimensions.get("confidence") {
            Some(DimensionValue::Number(value)) => *value,
            _ => 1.0,
        },
        extra,
    }
}

fn normalize_trust(value: Option<&DimensionValue>) -> TrustLevel {
    match value {
        Some(DimensionValue::Name(name)) => match name.as_str() {
            "autonomous" => TrustLevel::Autonomous,
            "supervisor_required" => TrustLevel::SupervisorRequired,
            "human_required" => TrustLevel::HumanRequired,
            other => TrustLevel::Custom(other.into()),
        },
        Some(DimensionValue::ConfidenceGated { above, .. }) => match above.as_str() {
            "autonomous" => TrustLevel::Autonomous,
            "supervisor_required" => TrustLevel::SupervisorRequired,
            "human_required" => TrustLevel::HumanRequired,
            other => TrustLevel::Custom(other.into()),
        },
        _ => TrustLevel::Autonomous,
    }
}

fn normalize_data(value: Option<&DimensionValue>) -> BTreeSet<DataCategory> {
    match value {
        Some(DimensionValue::Name(name)) if name != "none" => name
            .split(',')
            .map(|part| DataCategory(part.trim().to_string()))
            .collect(),
        _ => BTreeSet::new(),
    }
}

fn normalize_latency(value: Option<&DimensionValue>) -> LatencyLevel {
    match value {
        Some(DimensionValue::Name(name)) => match name.as_str() {
            "instant" => LatencyLevel::Instant,
            "fast" => LatencyLevel::Fast,
            "medium" => LatencyLevel::Medium,
            "slow" => LatencyLevel::Slow,
            other => LatencyLevel::Custom(other.into()),
        },
        Some(DimensionValue::Streaming { backpressure }) => LatencyLevel::Streaming {
            backpressure: backpressure.clone(),
        },
        _ => LatencyLevel::Instant,
    }
}

fn is_builtin_dimension(name: &str) -> bool {
    matches!(
        name,
        "cost" | "trust" | "reversible" | "data" | "latency" | "confidence"
    )
}

fn load_trace_events(path: &Path) -> Result<Vec<TraceEvent>> {
    let body = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read trace `{}`", path.display()))?;
    body.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).context("invalid trace JSONL event"))
        .collect()
}

fn collect_programs(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("cannot read corpus directory `{}`", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            out.extend(collect_programs(&path)?);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("cor") {
            out.push(path);
        }
    }
    Ok(out)
}

fn ensure_native_runtime_staticlib() -> Result<()> {
    static READY: OnceLock<Result<(), String>> = OnceLock::new();
    READY
        .get_or_init(|| {
            let output = Command::new("cargo")
                .arg("rustc")
                .arg("-p")
                .arg("corvid-runtime")
                .arg("--")
                .arg("--crate-type")
                .arg("staticlib")
                .current_dir(workspace_root())
                .output()
                .map_err(|err| format!("failed to spawn cargo rustc for corvid-runtime: {err}"))?;
            if output.status.success() {
                Ok(())
            } else {
                Err(format!(
                    "cargo rustc -p corvid-runtime -- --crate-type staticlib failed: stdout=`{}` stderr=`{}`",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                ))
            }
        })
        .clone()
        .map_err(|err| anyhow!(err))
}

fn persistent_verify_dir(prefix: &str) -> Result<PathBuf> {
    // Process id + wall-clock nanoseconds alone collide under parallel
    // test threads on Windows, where SystemTime::now() has ~100ns
    // resolution. Include the OS thread id and a process-wide atomic
    // counter so two concurrent verifier calls always land in distinct
    // directories. The counter advances monotonically even if the
    // clock doesn't.
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let unique = format!(
        "{prefix}-{}-{:?}-{}-{}",
        std::process::id(),
        std::thread::current().id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        seq
    );
    let dir = std::env::temp_dir().join("corvid-verify").join(unique);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create `{}`", dir.display()))?;
    Ok(dir)
}

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root from crate manifest path")
}

fn select_entry_agent(ir: &corvid_ir::IrFile) -> Result<String> {
    if ir.agents.len() == 1 {
        return Ok(ir.agents[0].name.clone());
    }
    ir.agents
        .iter()
        .find(|agent| agent.name == "main")
        .map(|agent| agent.name.clone())
        .ok_or_else(|| anyhow!("multiple agents declared and none named `main`"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .to_path_buf()
    }

    #[test]
    fn clean_corpus_program_has_no_divergence() {
        let report = verify_program(&repo_root().join("tests/corpus/cost_string.cor"))
            .expect("verify clean corpus file");
        assert!(
            report.divergences.is_empty(),
            "unexpected divergences: {:?}",
            report.divergences
        );
    }

    #[test]
    fn deliberate_fail_program_is_caught() {
        let report =
            verify_program(&repo_root().join("tests/corpus/should_fail/tier_disagree.cor"))
                .expect("verify deliberate fail fixture");
        assert!(!report.divergences.is_empty(), "expected divergence report");
    }

    #[test]
    fn native_drop_fixture_is_classified_as_too_loose() {
        let report =
            verify_program(&repo_root().join("tests/corpus/should_fail/native_drops_effect.cor"))
                .expect("verify native drop fixture");
        assert!(
            report.divergences.iter().any(|divergence| {
                divergence.dimension == "trust"
                    && divergence.classification == DivergenceClass::StaticTooLoose
            }),
            "expected native trust drop to classify as static-too-loose: {:?}",
            report.divergences
        );
    }

    #[test]
    fn corpus_scan_includes_deliberate_failure() {
        let reports = verify_corpus(&repo_root().join("tests/corpus")).expect("verify corpus");
        assert!(
            reports.iter().any(|report| !report.divergences.is_empty()),
            "expected at least one divergent corpus report"
        );
    }

    #[test]
    fn shrinker_writes_smaller_reproducer() {
        let fixture = repo_root().join("tests/corpus/should_fail/tier_disagree.cor");
        let result = shrink_program(&fixture).expect("shrink divergent program");
        let original = std::fs::read_to_string(&fixture).unwrap();
        let shrunk = std::fs::read_to_string(&result.output).unwrap();
        assert!(
            shrunk.lines().count() <= original.lines().count(),
            "shrinker must not grow the program"
        );
        let _ = std::fs::remove_file(result.output);
    }
}
