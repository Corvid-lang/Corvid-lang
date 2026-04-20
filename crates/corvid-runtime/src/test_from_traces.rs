use crate::errors::RuntimeError;
use crate::replay::{
    LlmDivergence, ReplayDifferentialReport, ReplayDivergence, RunCompletionDivergence,
    SubstitutionDivergence,
};
use crate::tracing::fresh_run_id;
use corvid_trace_schema::{read_events_from_path, validate_supported_schema, TraceEvent};
use std::collections::VecDeque;
use std::future::Future;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct TestFromTracesOptions {
    pub replay_model: Option<String>,
    pub promote: bool,
    pub flake_detect: Option<u32>,
    pub prompt_mode: PromotePromptMode,
}

impl Default for TestFromTracesOptions {
    fn default() -> Self {
        Self {
            replay_model: None,
            promote: false,
            flake_detect: None,
            prompt_mode: PromotePromptMode::AutoStdin,
        }
    }
}

#[derive(Debug, Clone)]
pub enum PromotePromptMode {
    AutoStdin,
    Decisions(Vec<PromoteDecision>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromoteDecision {
    AcceptOne,
    Reject,
    AcceptAllRemaining,
    Quit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Passed,
    Diverged,
    Flaky,
    Promoted,
    Error,
}

#[derive(Debug, Clone)]
pub enum Divergence {
    Replay(ReplayDivergence),
    DifferentialLlm(LlmDivergence),
    DifferentialSubstitution(SubstitutionDivergence),
    DifferentialRunCompletion(RunCompletionDivergence),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlakeRank {
    pub total_runs: u32,
    pub divergent_runs: u32,
}

#[derive(Debug, Clone)]
pub struct ModelSwapOutcome {
    pub model: String,
    pub report: ReplayDifferentialReport,
}

#[derive(Debug, Clone)]
pub struct TraceOutcome {
    pub path: PathBuf,
    pub run_id: String,
    pub verdict: Verdict,
    pub divergences: Vec<Divergence>,
    pub flake_rank: Option<FlakeRank>,
    pub promoted: bool,
    pub model_swap: Option<ModelSwapOutcome>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct TestFromTracesSummary {
    pub total: usize,
    pub passed: usize,
    pub diverged: usize,
    pub flaky: usize,
    pub promoted: usize,
    pub errored: usize,
}

#[derive(Debug, Clone, Default)]
pub struct TestFromTracesReport {
    pub per_trace: Vec<TraceOutcome>,
    pub summary: TestFromTracesSummary,
    pub aborted: bool,
}

#[derive(Debug, Clone)]
pub enum TraceHarnessMode {
    Replay,
    Differential { model: String },
    RecordCurrent,
}

#[derive(Debug, Clone)]
pub struct TraceHarnessRequest {
    pub trace_path: PathBuf,
    pub mode: TraceHarnessMode,
    pub emit_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct TraceHarnessRun {
    pub final_output: Option<serde_json::Value>,
    pub ok: bool,
    pub error: Option<String>,
    pub emitted_trace_path: PathBuf,
    pub differential_report: Option<ReplayDifferentialReport>,
}

#[derive(Debug, Clone)]
struct TraceMeta {
    run_id: String,
}

pub async fn run_test_from_traces<F, Fut>(
    trace_paths: Vec<PathBuf>,
    options: TestFromTracesOptions,
    mut runner: F,
) -> TestFromTracesReport
where
    F: FnMut(TraceHarnessRequest) -> Fut,
    Fut: Future<Output = Result<TraceHarnessRun, RuntimeError>>,
{
    let mut report = TestFromTracesReport::default();
    let mut promoter = Promoter::new(options.prompt_mode);
    report.summary.total = trace_paths.len();

    for trace_path in trace_paths {
        let meta = match load_trace_meta(&trace_path) {
            Ok(meta) => meta,
            Err(err) => {
                push_outcome(
                    &mut report,
                    TraceOutcome {
                        path: trace_path,
                        run_id: "<unknown>".into(),
                        verdict: Verdict::Error,
                        divergences: Vec::new(),
                        flake_rank: None,
                        promoted: false,
                        model_swap: None,
                        error: Some(err.to_string()),
                    },
                );
                continue;
            }
        };

        let outcome = if let Some(model) = &options.replay_model {
            run_differential(&trace_path, &meta, model, &mut runner).await
        } else if let Some(n) = options.flake_detect {
            run_flake_detect(&trace_path, &meta, n, &mut runner).await
        } else {
            run_plain_or_promote(
                &trace_path,
                &meta,
                options.promote,
                &mut promoter,
                &mut runner,
            )
            .await
        };

        match outcome {
            HarnessTraceResult::Outcome(outcome) => push_outcome(&mut report, outcome),
            HarnessTraceResult::Abort(outcome) => {
                push_outcome(&mut report, outcome);
                report.aborted = true;
                break;
            }
        }
    }

    report
}

enum HarnessTraceResult {
    Outcome(TraceOutcome),
    Abort(TraceOutcome),
}

async fn run_differential<F, Fut>(
    trace_path: &Path,
    meta: &TraceMeta,
    model: &str,
    runner: &mut F,
) -> HarnessTraceResult
where
    F: FnMut(TraceHarnessRequest) -> Fut,
    Fut: Future<Output = Result<TraceHarnessRun, RuntimeError>>,
{
    let request = TraceHarnessRequest {
        trace_path: trace_path.to_path_buf(),
        mode: TraceHarnessMode::Differential {
            model: model.to_string(),
        },
        emit_dir: fresh_emit_dir(),
    };
    match runner(request).await {
        Ok(run) => {
            let report = run.differential_report.unwrap_or_default();
            let divergences = differential_divergences(&report);
            let verdict = if report.is_empty() {
                Verdict::Passed
            } else {
                Verdict::Diverged
            };
            HarnessTraceResult::Outcome(TraceOutcome {
                path: trace_path.to_path_buf(),
                run_id: meta.run_id.clone(),
                verdict,
                divergences,
                flake_rank: None,
                promoted: false,
                model_swap: Some(ModelSwapOutcome {
                    model: model.to_string(),
                    report,
                }),
                error: run.error,
            })
        }
        Err(RuntimeError::ReplayDivergence(divergence)) => HarnessTraceResult::Outcome(TraceOutcome {
            path: trace_path.to_path_buf(),
            run_id: meta.run_id.clone(),
            verdict: Verdict::Diverged,
            divergences: vec![Divergence::Replay(divergence)],
            flake_rank: None,
            promoted: false,
            model_swap: Some(ModelSwapOutcome {
                model: model.to_string(),
                report: ReplayDifferentialReport::default(),
            }),
            error: None,
        }),
        Err(err) => HarnessTraceResult::Outcome(error_outcome(trace_path, meta, err)),
    }
}

async fn run_flake_detect<F, Fut>(
    trace_path: &Path,
    meta: &TraceMeta,
    runs: u32,
    runner: &mut F,
) -> HarnessTraceResult
where
    F: FnMut(TraceHarnessRequest) -> Fut,
    Fut: Future<Output = Result<TraceHarnessRun, RuntimeError>>,
{
    let mut baseline: Option<RunFingerprint> = None;
    let mut divergent_runs = 0u32;

    for _ in 0..runs {
        let request = TraceHarnessRequest {
            trace_path: trace_path.to_path_buf(),
            mode: TraceHarnessMode::Replay,
            emit_dir: fresh_emit_dir(),
        };
        match runner(request).await {
            Ok(run) => {
                let fingerprint = RunFingerprint::from_run(&run);
                match &baseline {
                    None => baseline = Some(fingerprint),
                    Some(first) if *first != fingerprint => divergent_runs += 1,
                    Some(_) => {}
                }
            }
            Err(RuntimeError::ReplayDivergence(divergence)) => {
                return HarnessTraceResult::Outcome(TraceOutcome {
                    path: trace_path.to_path_buf(),
                    run_id: meta.run_id.clone(),
                    verdict: Verdict::Diverged,
                    divergences: vec![Divergence::Replay(divergence)],
                    flake_rank: None,
                    promoted: false,
                    model_swap: None,
                    error: None,
                });
            }
            Err(err) => return HarnessTraceResult::Outcome(error_outcome(trace_path, meta, err)),
        }
    }

    HarnessTraceResult::Outcome(TraceOutcome {
        path: trace_path.to_path_buf(),
        run_id: meta.run_id.clone(),
        verdict: if divergent_runs == 0 {
            Verdict::Passed
        } else {
            Verdict::Flaky
        },
        divergences: Vec::new(),
        flake_rank: Some(FlakeRank {
            total_runs: runs,
            divergent_runs,
        }),
        promoted: false,
        model_swap: None,
        error: None,
    })
}

async fn run_plain_or_promote<F, Fut>(
    trace_path: &Path,
    meta: &TraceMeta,
    promote: bool,
    promoter: &mut Promoter,
    runner: &mut F,
) -> HarnessTraceResult
where
    F: FnMut(TraceHarnessRequest) -> Fut,
    Fut: Future<Output = Result<TraceHarnessRun, RuntimeError>>,
{
    let request = TraceHarnessRequest {
        trace_path: trace_path.to_path_buf(),
        mode: TraceHarnessMode::Replay,
        emit_dir: fresh_emit_dir(),
    };
    match runner(request).await {
        Ok(run) => HarnessTraceResult::Outcome(TraceOutcome {
            path: trace_path.to_path_buf(),
            run_id: meta.run_id.clone(),
            verdict: Verdict::Passed,
            divergences: Vec::new(),
            flake_rank: None,
            promoted: false,
            model_swap: None,
            error: run.error,
        }),
        Err(RuntimeError::ReplayDivergence(divergence)) if promote => {
            print_divergence_summary(trace_path, &[Divergence::Replay(divergence.clone())]);
            match promoter.decide() {
                Ok(PromoteDecision::Reject) => HarnessTraceResult::Outcome(TraceOutcome {
                    path: trace_path.to_path_buf(),
                    run_id: meta.run_id.clone(),
                    verdict: Verdict::Diverged,
                    divergences: vec![Divergence::Replay(divergence)],
                    flake_rank: None,
                    promoted: false,
                    model_swap: None,
                    error: None,
                }),
                Ok(PromoteDecision::AcceptOne) | Ok(PromoteDecision::AcceptAllRemaining) => {
                    let record_request = TraceHarnessRequest {
                        trace_path: trace_path.to_path_buf(),
                        mode: TraceHarnessMode::RecordCurrent,
                        emit_dir: fresh_emit_dir(),
                    };
                    match runner(record_request).await {
                        Ok(run) => match rewrite_trace_atomically(trace_path, &run.emitted_trace_path) {
                            Ok(()) => HarnessTraceResult::Outcome(TraceOutcome {
                                path: trace_path.to_path_buf(),
                                run_id: meta.run_id.clone(),
                                verdict: Verdict::Promoted,
                                divergences: vec![Divergence::Replay(divergence)],
                                flake_rank: None,
                                promoted: true,
                                model_swap: None,
                                error: run.error,
                            }),
                            Err(err) => HarnessTraceResult::Outcome(error_outcome(
                                trace_path,
                                meta,
                                RuntimeError::Other(err.to_string()),
                            )),
                        },
                        Err(err) => HarnessTraceResult::Outcome(error_outcome(trace_path, meta, err)),
                    }
                }
                Ok(PromoteDecision::Quit) => HarnessTraceResult::Abort(TraceOutcome {
                    path: trace_path.to_path_buf(),
                    run_id: meta.run_id.clone(),
                    verdict: Verdict::Error,
                    divergences: vec![Divergence::Replay(divergence)],
                    flake_rank: None,
                    promoted: false,
                    model_swap: None,
                    error: Some("promotion aborted by user".into()),
                }),
                Err(err) => HarnessTraceResult::Outcome(error_outcome(
                    trace_path,
                    meta,
                    RuntimeError::Other(err.to_string()),
                )),
            }
        }
        Err(RuntimeError::ReplayDivergence(divergence)) => HarnessTraceResult::Outcome(TraceOutcome {
            path: trace_path.to_path_buf(),
            run_id: meta.run_id.clone(),
            verdict: Verdict::Diverged,
            divergences: vec![Divergence::Replay(divergence)],
            flake_rank: None,
            promoted: false,
            model_swap: None,
            error: None,
        }),
        Err(err) => HarnessTraceResult::Outcome(error_outcome(trace_path, meta, err)),
    }
}

fn load_trace_meta(path: &Path) -> Result<TraceMeta, RuntimeError> {
    let events = read_events_from_path(path).map_err(|err| RuntimeError::ReplayTraceLoad {
        path: path.to_path_buf(),
        message: err.to_string(),
    })?;
    validate_supported_schema(&events).map_err(|err| RuntimeError::ReplayTraceLoad {
        path: path.to_path_buf(),
        message: err.to_string(),
    })?;
    let run_id = events
        .iter()
        .find_map(|event| match event {
            TraceEvent::SchemaHeader { run_id, .. }
            | TraceEvent::RunStarted { run_id, .. }
            | TraceEvent::RunCompleted { run_id, .. } => Some(run_id.clone()),
            _ => None,
        })
        .ok_or_else(|| RuntimeError::ReplayTraceLoad {
            path: path.to_path_buf(),
            message: "trace missing run_id-bearing events".into(),
        })?;
    Ok(TraceMeta { run_id })
}

fn fresh_emit_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("corvid-harness-{}", fresh_run_id()));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn rewrite_trace_atomically(original: &Path, replacement: &Path) -> io::Result<()> {
    let parent = original.parent().unwrap_or_else(|| Path::new("."));
    let temp = parent.join(format!(
        ".{}.rewrite-{}.tmp",
        original
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("trace"),
        fresh_run_id()
    ));
    std::fs::copy(replacement, &temp)?;
    std::fs::rename(&temp, original)?;
    Ok(())
}

fn differential_divergences(report: &ReplayDifferentialReport) -> Vec<Divergence> {
    let mut out = Vec::new();
    out.extend(
        report
            .llm_divergences
            .iter()
            .cloned()
            .map(Divergence::DifferentialLlm),
    );
    out.extend(
        report
            .substitution_divergences
            .iter()
            .cloned()
            .map(Divergence::DifferentialSubstitution),
    );
    if let Some(run_completion) = &report.run_completion_divergence {
        out.push(Divergence::DifferentialRunCompletion(run_completion.clone()));
    }
    out
}

fn error_outcome(path: &Path, meta: &TraceMeta, err: RuntimeError) -> TraceOutcome {
    TraceOutcome {
        path: path.to_path_buf(),
        run_id: meta.run_id.clone(),
        verdict: Verdict::Error,
        divergences: Vec::new(),
        flake_rank: None,
        promoted: false,
        model_swap: None,
        error: Some(err.to_string()),
    }
}

fn push_outcome(report: &mut TestFromTracesReport, outcome: TraceOutcome) {
    match outcome.verdict {
        Verdict::Passed => report.summary.passed += 1,
        Verdict::Diverged => report.summary.diverged += 1,
        Verdict::Flaky => report.summary.flaky += 1,
        Verdict::Promoted => report.summary.promoted += 1,
        Verdict::Error => report.summary.errored += 1,
    }
    report.per_trace.push(outcome);
}

fn print_divergence_summary(path: &Path, divergences: &[Divergence]) {
    eprintln!("trace `{}` diverged:", path.display());
    for divergence in divergences {
        match divergence {
            Divergence::Replay(div) => {
                eprintln!("  step {} {} {}", div.step, div.got_kind, div.got_description);
            }
            Divergence::DifferentialLlm(div) => {
                eprintln!("  step {} llm `{}` {:?} -> {:?}", div.step, div.prompt, div.recorded, div.live);
            }
            Divergence::DifferentialSubstitution(div) => {
                eprintln!("  step {} substitution {}", div.step, div.got_description);
            }
            Divergence::DifferentialRunCompletion(div) => {
                eprintln!("  run completion at step {} changed", div.step);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct RunFingerprint {
    ok: bool,
    final_output: Option<serde_json::Value>,
    error: Option<String>,
}

impl RunFingerprint {
    fn from_run(run: &TraceHarnessRun) -> Self {
        Self {
            ok: run.ok,
            final_output: run.final_output.clone(),
            error: run.error.clone(),
        }
    }
}

struct Promoter {
    mode: PromotePromptMode,
    accept_all_remaining: bool,
    warned_noninteractive: bool,
    scripted: VecDeque<PromoteDecision>,
}

impl Promoter {
    fn new(mode: PromotePromptMode) -> Self {
        let scripted = match &mode {
            PromotePromptMode::Decisions(decisions) => decisions.clone().into(),
            PromotePromptMode::AutoStdin => VecDeque::new(),
        };
        Self {
            mode,
            accept_all_remaining: false,
            warned_noninteractive: false,
            scripted,
        }
    }

    fn decide(&mut self) -> io::Result<PromoteDecision> {
        if self.accept_all_remaining {
            return Ok(PromoteDecision::AcceptAllRemaining);
        }
        if let Some(decision) = self.scripted.pop_front() {
            if decision == PromoteDecision::AcceptAllRemaining {
                self.accept_all_remaining = true;
            }
            return Ok(decision);
        }
        match self.mode {
            PromotePromptMode::Decisions(_) => Ok(PromoteDecision::Reject),
            PromotePromptMode::AutoStdin => self.read_stdin_decision(),
        }
    }

    fn read_stdin_decision(&mut self) -> io::Result<PromoteDecision> {
        if !io::stdin().is_terminal() {
            if !self.warned_noninteractive {
                eprintln!(
                    "note: stdin is not a TTY; defaulting all `--promote` prompts to `N` for CI safety"
                );
                self.warned_noninteractive = true;
            }
            return Ok(PromoteDecision::Reject);
        }

        eprint!("promote? [y/N/a/q]: ");
        io::stderr().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let trimmed = input.trim().to_ascii_lowercase();
        let decision = match trimmed.as_str() {
            "y" => PromoteDecision::AcceptOne,
            "a" => {
                self.accept_all_remaining = true;
                PromoteDecision::AcceptAllRemaining
            }
            "q" => PromoteDecision::Quit,
            _ => PromoteDecision::Reject,
        };
        Ok(decision)
    }
}
