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

pub mod fuzz;

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
    files.into_iter().map(|path| verify_program(&path)).collect()
}

pub fn shrink_program(path: &Path) -> Result<ShrinkResult> {
    let original = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read `{}`", path.display()))?;
    let report = verify_program(path)?;
    if report.divergences.is_empty() {
        bail!("`{}` does not diverge; nothing to shrink", path.display());
    }

    let mut lines: Vec<String> = original.lines().map(|line| line.to_string()).collect();
    let mut changed = true;
    while changed {
        changed = false;
        for idx in 0..lines.len() {
            let line = lines[idx].trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut candidate = lines.clone();
            candidate.remove(idx);
            let candidate_source = candidate.join("\n");
            let tmp = tempfile::NamedTempFile::new()
                .context("failed to create shrink candidate file")?;
            std::fs::write(tmp.path(), &candidate_source).context("failed to write candidate")?;
            let candidate_report = verify_program(tmp.path());
            if let Ok(candidate_report) = candidate_report {
                if !candidate_report.divergences.is_empty() {
                    lines = candidate;
                    changed = true;
                    break;
                }
            }
        }
    }

    let output = path.with_file_name(format!(
        "{}.shrunk.cor",
        path.file_stem().and_then(|stem| stem.to_str()).unwrap_or("reproducer")
    ));
    std::fs::write(&output, lines.join("\n"))
        .with_context(|| format!("failed to write `{}`", output.display()))?;
    Ok(ShrinkResult {
        original: path.to_path_buf(),
        output,
        removed_lines: original.lines().count().saturating_sub(lines.len()),
    })
}

pub fn render_corpus_grid(reports: &[DivergenceReport]) -> String {
    let mut lines = vec![
        format!(
            "{:<34} {:<7} {:<7} {:<7} {:<7} {:<9}",
            "program", "check", "interp", "native", "replay", "verdict"
        ),
        format!(
            "{:-<34} {:-<7} {:-<7} {:-<7} {:-<7} {:-<9}",
            "", "", "", "", "", ""
        ),
    ];
    for report in reports {
        let interp = &report.reports[1].profile;
        let cells: Vec<_> = report.reports.iter().map(|tier| {
            if tier.profile == *interp { "agree" } else { "diff" }
        }).collect();
        let verdict = if report.divergences.is_empty() {
            "ok"
        } else {
            "diverges"
        };
        lines.push(format!(
            "{:<34} {:<7} {:<7} {:<7} {:<7} {:<9}",
            report
                .program
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("<unknown>"),
            cells[0],
            cells[1],
            cells[2],
            cells[3],
            verdict
        ));
    }
    lines.join("\n")
}

pub fn render_report(report: &DivergenceReport) -> String {
    let mut lines = vec![format!("{}", report.program.display())];
    for tier in &report.reports {
        lines.push(format!(
            "  {:<11} cost=${:.4} trust={} reversible={} data={} latency={} confidence={:.2}",
            format!("{:?}", tier.tier).to_lowercase(),
            tier.profile.cost,
            render_trust(&tier.profile.trust),
            tier.profile.reversible,
            render_data(&tier.profile.data),
            render_latency(&tier.profile.latency),
            tier.profile.confidence,
        ));
    }
    if report.divergences.is_empty() {
        lines.push("  divergences: none".into());
    } else {
        lines.push(format!("  divergences: {}", report.divergences.len()));
        for divergence in &report.divergences {
            lines.push(format!(
                "    {} [{}]",
                divergence.dimension,
                render_divergence_class(&divergence.classification)
            ));
        }
    }
    lines.join("\n")
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
            bail!("resolve failed for `{}`: {:?}", path.display(), resolved.errors);
        }
        let checked = typecheck(&file, &resolved);
        if !checked.errors.is_empty() {
            bail!("typecheck failed for `{}`: {:?}", path.display(), checked.errors);
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
    let binary = build_native_to_disk(&frontend.ir, "corvid_verify", &bin_path, &[])
        .map_err(|err| anyhow!("native build failed for `{}`: {err}", frontend.path.display()))?;
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
                mock_value_for_type(&prompt.return_ty)
                    .with_context(|| format!("unsupported prompt return type in `{}`", frontend.path.display()))?,
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

fn profile_from_trace(frontend: &Frontend, events: &[TraceEvent]) -> Result<(EffectProfile, Vec<String>)> {
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

fn diff_reports(reports: &[TierReport; 4]) -> Result<Vec<Divergence>> {
    let mut divergences = Vec::new();

    maybe_push_divergence(
        &mut divergences,
        "cost",
        value_map(reports, |profile| serde_json::json!(profile.cost)),
        reports.iter().all(|report| report.profile.cost == reports[0].profile.cost),
        reports,
    )?;
    maybe_push_divergence(
        &mut divergences,
        "trust",
        value_map(reports, |profile| serde_json::json!(render_trust(&profile.trust))),
        reports
            .iter()
            .all(|report| report.profile.trust == reports[0].profile.trust),
        reports,
    )?;
    maybe_push_divergence(
        &mut divergences,
        "reversible",
        value_map(reports, |profile| serde_json::json!(profile.reversible)),
        reports
            .iter()
            .all(|report| report.profile.reversible == reports[0].profile.reversible),
        reports,
    )?;
    maybe_push_divergence(
        &mut divergences,
        "data",
        value_map(reports, |profile| serde_json::json!(render_data(&profile.data))),
        reports
            .iter()
            .all(|report| report.profile.data == reports[0].profile.data),
        reports,
    )?;
    maybe_push_divergence(
        &mut divergences,
        "latency",
        value_map(reports, |profile| serde_json::json!(render_latency(&profile.latency))),
        reports
            .iter()
            .all(|report| report.profile.latency == reports[0].profile.latency),
        reports,
    )?;
    maybe_push_divergence(
        &mut divergences,
        "confidence",
        value_map(reports, |profile| serde_json::json!(profile.confidence)),
        reports
            .iter()
            .all(|report| report.profile.confidence == reports[0].profile.confidence),
        reports,
    )?;

    let extra_keys: BTreeSet<_> = reports
        .iter()
        .flat_map(|report| report.profile.extra.keys().cloned())
        .collect();
    for key in extra_keys {
        let values = value_map(reports, |profile| {
            serde_json::to_value(profile.extra.get(&key)).unwrap_or(serde_json::Value::Null)
        });
        let all_equal = values.values().all(|value| value == values.get(&Tier::Checker).unwrap());
        if !all_equal {
            divergences.push(Divergence {
                dimension: key.clone(),
                classification: DivergenceClass::TierMismatch,
                values,
                attribution: divergent_tier_blame("extra")?,
            });
        }
    }

    Ok(divergences)
}

fn maybe_push_divergence(
    divergences: &mut Vec<Divergence>,
    dimension: &str,
    values: BTreeMap<Tier, serde_json::Value>,
    all_equal: bool,
    reports: &[TierReport; 4],
) -> Result<()> {
    if all_equal {
        return Ok(());
    }
    let classification = if has_tier_overapproximation(dimension, reports) {
        DivergenceClass::StaticOverapproximated
    } else if has_tier_too_loose(dimension, reports) {
        DivergenceClass::StaticTooLoose
    } else {
        DivergenceClass::TierMismatch
    };
    divergences.push(Divergence {
        dimension: dimension.into(),
        classification,
        values,
        attribution: divergent_tier_blame(dimension)?,
    });
    Ok(())
}

fn has_tier_overapproximation(dimension: &str, reports: &[TierReport; 4]) -> bool {
    let interpreter = &reports[1].profile;
    reports
        .iter()
        .enumerate()
        .filter(|(idx, _)| *idx != 1)
        .any(|(_, report)| is_profile_overapproximation(dimension, &report.profile, interpreter))
}

fn has_tier_too_loose(dimension: &str, reports: &[TierReport; 4]) -> bool {
    let interpreter = &reports[1].profile;
    reports
        .iter()
        .enumerate()
        .filter(|(idx, _)| *idx != 1)
        .any(|(_, report)| is_profile_too_loose(dimension, &report.profile, interpreter))
}

fn value_map(
    reports: &[TierReport; 4],
    value_of: impl Fn(&EffectProfile) -> serde_json::Value,
) -> BTreeMap<Tier, serde_json::Value> {
    reports
        .iter()
        .map(|report| (report.tier, value_of(&report.profile)))
        .collect()
}

fn is_profile_overapproximation(
    dimension: &str,
    candidate: &EffectProfile,
    baseline: &EffectProfile,
) -> bool {
    match dimension {
        "cost" => candidate.cost > baseline.cost,
        "trust" => trust_rank(&candidate.trust) > trust_rank(&baseline.trust),
        "reversible" => !candidate.reversible && baseline.reversible,
        "data" => candidate.data.is_superset(&baseline.data) && candidate.data != baseline.data,
        "latency" => latency_rank(&candidate.latency) > latency_rank(&baseline.latency),
        "confidence" => candidate.confidence < baseline.confidence,
        _ => false,
    }
}

fn is_profile_too_loose(
    dimension: &str,
    candidate: &EffectProfile,
    baseline: &EffectProfile,
) -> bool {
    match dimension {
        "cost" => candidate.cost < baseline.cost,
        "trust" => trust_rank(&candidate.trust) < trust_rank(&baseline.trust),
        "reversible" => candidate.reversible && !baseline.reversible,
        "data" => !candidate.data.is_superset(&baseline.data),
        "latency" => latency_rank(&candidate.latency) < latency_rank(&baseline.latency),
        "confidence" => candidate.confidence > baseline.confidence,
        _ => false,
    }
}

fn divergent_tier_blame(dimension: &str) -> Result<Vec<BlameAttribution>> {
    let targets = match dimension {
        "cost" | "trust" | "reversible" | "data" | "latency" | "confidence" => vec![
            (Tier::Checker, PathBuf::from("crates/corvid-types/src/effects.rs")),
            (Tier::Interpreter, PathBuf::from("crates/corvid-vm/src/interp.rs")),
            (Tier::Native, PathBuf::from("crates/corvid-runtime/src/ffi_bridge.rs")),
            (Tier::Replay, PathBuf::from("crates/corvid-runtime/src/tracing.rs")),
        ],
        _ => vec![(Tier::Checker, PathBuf::from("crates/corvid-types/src/effects.rs"))],
    };

    let mut attributions = Vec::new();
    for (tier, file) in targets {
        if let Some((commit, author)) = blame_file(&file)? {
            attributions.push(BlameAttribution {
                tier,
                file,
                commit,
                author,
            });
        }
    }
    Ok(attributions)
}

fn blame_file(path: &Path) -> Result<Option<(String, String)>> {
    let output = Command::new("git")
        .arg("blame")
        .arg("-L")
        .arg("1,1")
        .arg("--porcelain")
        .arg(path)
        .output()
        .with_context(|| format!("failed to run git blame for `{}`", path.display()))?;
    if !output.status.success() {
        return Ok(None);
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut commit = String::new();
    let mut author = String::new();
    for (index, line) in stdout.lines().enumerate() {
        if index == 0 {
            commit = line.split_whitespace().next().unwrap_or("").to_string();
        } else if let Some(rest) = line.strip_prefix("author ") {
            author = rest.to_string();
            break;
        }
    }
    if commit.is_empty() || author.is_empty() {
        Ok(None)
    } else {
        Ok(Some((commit, author)))
    }
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

fn render_trust(level: &TrustLevel) -> String {
    match level {
        TrustLevel::Autonomous => "autonomous".into(),
        TrustLevel::SupervisorRequired => "supervisor_required".into(),
        TrustLevel::HumanRequired => "human_required".into(),
        TrustLevel::Custom(name) => name.clone(),
    }
}

fn render_data(data: &BTreeSet<DataCategory>) -> String {
    if data.is_empty() {
        "none".into()
    } else {
        data.iter()
            .map(|category| category.0.as_str())
            .collect::<Vec<_>>()
            .join(",")
    }
}

fn render_latency(latency: &LatencyLevel) -> String {
    match latency {
        LatencyLevel::Instant => "instant".into(),
        LatencyLevel::Fast => "fast".into(),
        LatencyLevel::Medium => "medium".into(),
        LatencyLevel::Slow => "slow".into(),
        LatencyLevel::Streaming { backpressure } => match backpressure {
            BackpressurePolicy::Bounded(size) => format!("streaming(bounded({size}))"),
            BackpressurePolicy::Unbounded => "streaming(unbounded)".into(),
        },
        LatencyLevel::Custom(name) => name.clone(),
    }
}

fn render_divergence_class(class: &DivergenceClass) -> &'static str {
    match class {
        DivergenceClass::StaticOverapproximated => "static-overapprox",
        DivergenceClass::StaticTooLoose => "static-too-loose",
        DivergenceClass::TierMismatch => "tier-mismatch",
    }
}

fn trust_rank(level: &TrustLevel) -> u8 {
    match level {
        TrustLevel::Autonomous => 0,
        TrustLevel::SupervisorRequired => 1,
        TrustLevel::HumanRequired => 2,
        TrustLevel::Custom(_) => 3,
    }
}

fn latency_rank(level: &LatencyLevel) -> u8 {
    match level {
        LatencyLevel::Instant => 0,
        LatencyLevel::Fast => 1,
        LatencyLevel::Medium => 2,
        LatencyLevel::Slow => 3,
        LatencyLevel::Streaming { .. } => 4,
        LatencyLevel::Custom(_) => 5,
    }
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
        let report = verify_program(&repo_root().join("tests/corpus/should_fail/tier_disagree.cor"))
            .expect("verify deliberate fail fixture");
        assert!(
            !report.divergences.is_empty(),
            "expected divergence report"
        );
    }

    #[test]
    fn native_drop_fixture_is_classified_as_too_loose() {
        let report = verify_program(
            &repo_root().join("tests/corpus/should_fail/native_drops_effect.cor"),
        )
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
