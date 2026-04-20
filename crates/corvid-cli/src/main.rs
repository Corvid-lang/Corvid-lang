//! The `corvid` CLI.
//!
//! Subcommands:
//!   corvid new <name>         scaffold a new project
//!   corvid check <file>       type-check a source file
//!   corvid build <file>       compile to target/py/<name>.py
//!   corvid run <file>         build + invoke python on the output
//!   corvid repl               start the interactive REPL
//!   corvid test <what>        run verification suites (dimensions, spec, adversarial)
//!   corvid verify             cross-tier effect-profile verification
//!   corvid effect-diff        diff composed effect profiles between two revisions
//!   corvid add-dimension      install a dimension from the effect registry
//!   corvid routing-report     aggregate dispatch traces into routing guidance
//!   corvid replay <trace>     re-execute a recorded trace deterministically
//!   corvid replay --model <id> <trace>  differential replay against a different model
//!   corvid trace list         list traces under target/trace/
//!   corvid trace show <id>    print a recorded trace as formatted JSON
//!   corvid trace dag <id>     render provenance DAG as Graphviz DOT

mod replay;
mod routing_report;
mod test_from_traces;
mod trace_cmd;
mod trace_dag;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use corvid_differential_verify::{
    render_corpus_grid, render_report, shrink_program, verify_corpus,
};
use routing_report::{build_report, render_report as render_routing_report, RoutingReportOptions};

#[allow(unused_imports)]
use corvid_driver::{
    build_native_to_disk, build_to_disk, compile, compile_with_config, diff_snapshots,
    load_corvid_config_for, load_dotenv_walking, render_all_pretty, render_effect_diff,
    render_law_check_report, render_spec_report, run_law_checks, run_native, run_with_target,
    scaffold_new, snapshot_revision, verify_spec_examples, RunTarget, VerdictKind,
    DEFAULT_SAMPLES,
};

#[derive(Parser)]
#[command(name = "corvid", version, about = "The Corvid language compiler")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Scaffold a new Corvid project.
    New { name: String },
    /// Type-check a Corvid source file.
    Check { file: PathBuf },
    /// Compile a Corvid source file. Default target is Python (target/py/);
    /// `--target=native` emits a machine-code binary under target/bin/.
    Build {
        file: PathBuf,
        /// Output target. `python` (default) or `native`.
        #[arg(long, default_value = "python")]
        target: String,
    },
    /// Build and run a Corvid source file. Picks the native AOT tier
    /// when the program stays within the current native command-line
    /// boundary; falls back to the interpreter otherwise with a one-line
    /// notice. Override with `--target`.
    Run {
        file: PathBuf,
        /// Execution tier. `auto` (default) tries native first, falls
        /// back to interpreter when a feature isn't native-able yet.
        /// `native` requires native and errors out otherwise. `interp`
        /// / `interpreter` forces the interpreter tier.
        #[arg(long, default_value = "auto")]
        target: String,
        /// Path to a compiled `#[tool]` staticlib. When
        /// provided, tool-using programs compile and run natively; the
        /// linker resolves `__corvid_tool_<name>` symbols against this
        /// lib. Without it, tool-using programs fall back to the
        /// interpreter (auto) or error out (native).
        #[arg(long, value_name = "PATH")]
        with_tools_lib: Option<PathBuf>,
    },
    /// Run verification suites.
    ///
    /// Targets:
    ///   `dimensions`    algebraic-law proptest over every custom dimension
    ///                   declared in corvid.toml
    ///   `spec`          recompile every .cor example in docs/effects-spec/
    ///                   against the current toolchain; with --meta, run the
    ///                   self-verifying verification harness
    ///   `adversarial`   LLM-driven bypass generation against the effect
    ///                   checker
    ///
    /// Without a target, acts as a placeholder for the future unit-test
    /// runner.
    Test {
        /// What to verify. Omit for the legacy placeholder behavior.
        /// Mutually exclusive with `--from-traces`.
        #[arg(conflicts_with = "from_traces")]
        target: Option<String>,
        /// For `spec`: run the meta-verification harness (mutate the
        /// verifier, confirm each counter-example is still caught).
        #[arg(long)]
        meta: bool,
        /// For `adversarial`: number of bypass programs to generate.
        #[arg(long, default_value = "100")]
        count: u32,
        /// For `adversarial`: model to drive the generator.
        #[arg(long, default_value = "opus")]
        model: String,
        /// Prod-as-test-suite mode (Phase 21 slice 21-inv-G-cli).
        /// Replay every `.jsonl` in `<DIR>` against the current code
        /// and report any behavior drift. Today's stub loads,
        /// validates, filters, and reports coverage; the live
        /// regression harness ships in Dev B's `21-inv-G-harness`.
        #[arg(long, value_name = "DIR")]
        from_traces: Option<PathBuf>,
        /// For `--from-traces`: differential replay against a
        /// different model. Composes with `21-inv-B-adapter`. When
        /// present, every trace's recorded LLM results are compared
        /// against this model's live output; divergences surface in
        /// the regression report.
        #[arg(long, value_name = "ID", requires = "from_traces")]
        replay_model: Option<String>,
        /// For `--from-traces`: only include traces that hit a
        /// `@dangerous` tool. The Corvid approve-before-dangerous
        /// guarantee means traces with an `ApprovalRequest` event
        /// are exactly the dangerous-tool traces; no separate
        /// annotation needed.
        #[arg(long, requires = "from_traces")]
        only_dangerous: bool,
        /// For `--from-traces`: only include traces that exercise
        /// the named prompt.
        #[arg(long, value_name = "NAME", requires = "from_traces")]
        only_prompt: Option<String>,
        /// For `--from-traces`: only include traces that exercise
        /// the named tool.
        #[arg(long, value_name = "NAME", requires = "from_traces")]
        only_tool: Option<String>,
        /// For `--from-traces`: only include traces with at least
        /// one event at or after this RFC3339 timestamp (matches
        /// `corvid routing-report --since`).
        #[arg(long, value_name = "RFC3339", requires = "from_traces")]
        since: Option<String>,
        /// For `--from-traces`: promote mode. Divergences become
        /// interactively-accepted "golden" traces, overwriting the
        /// originals (Jest-snapshot-style). Mutually exclusive with
        /// `--replay-model` (promoting cross-model divergences would
        /// quietly steal your golden's model; re-record instead)
        /// and with `--flake-detect` (promoting a flaky result is
        /// a bug).
        #[arg(
            long,
            requires = "from_traces",
            conflicts_with = "replay_model",
            conflicts_with = "flake_detect",
        )]
        promote: bool,
        /// For `--from-traces`: flake-detection mode. Replay each
        /// trace N times; any trace producing different output
        /// across runs surfaces program-level nondeterminism the
        /// `@deterministic` attribute didn't catch. Since replay
        /// substitutes recorded responses, deterministic programs
        /// must produce identical output every time.
        #[arg(long, value_name = "N", requires = "from_traces")]
        flake_detect: Option<u32>,
    },
    /// Cross-verify effect profiles across checker, interpreter,
    /// native, and replay tiers.
    Verify {
        /// Verify every `.cor` file under this directory recursively.
        #[arg(long, value_name = "DIR", conflicts_with = "shrink")]
        corpus: Option<PathBuf>,
        /// Shrink a divergent program to a smaller reproducer.
        #[arg(long, value_name = "FILE", conflicts_with = "corpus")]
        shrink: Option<PathBuf>,
        /// Emit the structured report as JSON to stderr.
        #[arg(long)]
        json: bool,
    },
    /// Diff the composed effect profile between two revisions.
    /// Reports dimension-value drift per agent and constraints that
    /// newly fire or release because of the change.
    EffectDiff {
        /// Before revision (git ref or file path).
        before: String,
        /// After revision (git ref or file path).
        after: String,
    },
    /// Install a dimension from the Corvid effect registry.
    /// Verifies the dimension's signature, replays its algebraic-law
    /// proofs against the current toolchain, adds it to corvid.toml.
    AddDimension {
        /// Dimension spec in `name@version` form (e.g. `fairness@1.2`).
        spec: String,
    },
    /// Aggregate routing and dispatch traces into an optimization report.
    RoutingReport {
        /// Only include events at or after this RFC3339 timestamp.
        #[arg(long)]
        since: Option<String>,
        /// Only include events at or after this git commit timestamp.
        #[arg(long)]
        since_commit: Option<String>,
        /// Emit the structured report as JSON.
        #[arg(long)]
        json: bool,
        /// Trace directory. Defaults to `target/trace`.
        #[arg(long, value_name = "PATH")]
        trace_dir: Option<PathBuf>,
    },
    /// Re-execute a recorded trace deterministically.
    ///
    /// Default mode: substitute recorded responses for every
    /// live call and reproduce the original run byte-for-byte.
    ///
    /// With `--model <id>`: differential replay — instead of
    /// using recorded LLM results verbatim, issue each prompt
    /// against `<id>` and render a divergence report listing
    /// the steps whose output differs. Useful for testing a
    /// new model version against a corpus of prod traces
    /// without paying the full re-run cost.
    ///
    /// With `--mutate <STEP> <JSON>`: counterfactual replay —
    /// replay the trace with exactly one recorded response
    /// overridden and render the downstream behavior diff.
    /// `<STEP>` is the 1-based index among substitutable events
    /// (ToolCall / LlmCall / ApprovalRequest) as numbered in
    /// `corvid trace show`.
    ///
    /// `--source <FILE>` points at the Corvid source the trace
    /// was recorded against. Eventually this will be inferred
    /// from `SchemaHeader.source_path` when populated; today
    /// it's required for any actually-execute mode (differential
    /// / plain). Modes that don't actually run (load-validation
    /// only) ignore it.
    Replay {
        /// Path to a JSONL trace file.
        trace: PathBuf,
        /// Path to the Corvid source the trace was recorded
        /// against. Required for modes that execute (default
        /// plain replay, `--model` differential, `--mutate`
        /// counterfactual). Once `SchemaHeader.source_path` is
        /// populated at record time, this flag becomes optional.
        #[arg(long, value_name = "FILE")]
        source: Option<PathBuf>,
        /// Target model for differential replay. When present,
        /// replays against this model and reports divergences
        /// against the recorded results. When absent, runs a
        /// plain reproduction. Mutually exclusive with
        /// `--mutate`.
        #[arg(long, value_name = "ID", conflicts_with = "mutate")]
        model: Option<String>,
        /// Counterfactual replay: replace the response of the
        /// substitutable event at 1-based `STEP` with `JSON`,
        /// then replay and diff. Mutually exclusive with
        /// `--model`.
        #[arg(
            long,
            num_args = 2,
            value_names = ["STEP", "JSON"],
            conflicts_with = "model",
        )]
        mutate: Option<Vec<String>>,
    },
    /// Inspect recorded traces under `target/trace/` (or a
    /// user-supplied directory).
    Trace {
        #[command(subcommand)]
        command: TraceCommand,
    },
    /// Start the interactive Corvid REPL.
    Repl,
    /// Check the local environment for required tools.
    Doctor,
}

#[derive(Subcommand)]
enum TraceCommand {
    /// List every JSONL trace under `--trace-dir` (default:
    /// `target/trace/`). One row per trace with run id, schema
    /// version, event count, and timestamp range.
    List {
        /// Trace directory. Defaults to `target/trace`.
        #[arg(long, value_name = "PATH")]
        trace_dir: Option<PathBuf>,
    },
    /// Print every event in a trace as formatted JSON, one
    /// event per line.
    Show {
        /// Trace identifier: either a direct file path, or a
        /// run id to resolve under `--trace-dir`.
        id_or_path: String,
        /// Trace directory used when `id_or_path` is a bare
        /// run id. Defaults to `target/trace`.
        #[arg(long, value_name = "PATH")]
        trace_dir: Option<PathBuf>,
    },
    /// Render the Grounded<T> provenance DAG of a trace as a
    /// Graphviz DOT graph. Pipe into `dot -Tsvg > prov.svg` to
    /// render. Traces without provenance events produce an empty
    /// digraph plus a warning on stderr.
    Dag {
        /// Trace identifier: either a direct file path, or a
        /// run id to resolve under `--trace-dir`.
        id_or_path: String,
        /// Trace directory used when `id_or_path` is a bare
        /// run id. Defaults to `target/trace`.
        #[arg(long, value_name = "PATH")]
        trace_dir: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let result = match cli.command {
        Some(Command::New { name }) => cmd_new(&name),
        Some(Command::Check { file }) => cmd_check(&file),
        Some(Command::Build { file, target }) => cmd_build(&file, &target),
        Some(Command::Run {
            file,
            target,
            with_tools_lib,
        }) => cmd_run(&file, &target, with_tools_lib.as_deref()),
        Some(Command::Test {
            target,
            meta,
            count,
            model,
            from_traces,
            replay_model,
            only_dangerous,
            only_prompt,
            only_tool,
            since,
            promote,
            flake_detect,
        }) => {
            if let Some(dir) = from_traces {
                test_from_traces::run_test_from_traces(
                    test_from_traces::TestFromTracesArgs {
                        trace_dir: &dir,
                        replay_model: replay_model.as_deref(),
                        only_dangerous,
                        only_prompt: only_prompt.as_deref(),
                        only_tool: only_tool.as_deref(),
                        since: since.as_deref(),
                        promote,
                        flake_detect,
                    },
                )
            } else {
                cmd_test(target.as_deref(), meta, count, &model)
            }
        }
        Some(Command::Verify { corpus, shrink, json }) => {
            cmd_verify(corpus.as_deref(), shrink.as_deref(), json)
        }
        Some(Command::EffectDiff { before, after }) => cmd_effect_diff(&before, &after),
        Some(Command::AddDimension { spec }) => cmd_add_dimension(&spec),
        Some(Command::RoutingReport {
            since,
            since_commit,
            json,
            trace_dir,
        }) => cmd_routing_report(
            trace_dir.as_deref(),
            since.as_deref(),
            since_commit.as_deref(),
            json,
        ),
        Some(Command::Replay {
            trace,
            source,
            model,
            mutate,
        }) => replay::run_replay(
            &trace,
            source.as_deref(),
            model.as_deref(),
            mutate.as_deref(),
        ),
        Some(Command::Trace { command }) => match command {
            TraceCommand::List { trace_dir } => trace_cmd::run_list(trace_dir.as_deref()),
            TraceCommand::Show {
                id_or_path,
                trace_dir,
            } => trace_cmd::run_show(&id_or_path, trace_dir.as_deref()),
            TraceCommand::Dag {
                id_or_path,
                trace_dir,
            } => trace_dag::run_dag(&id_or_path, trace_dir.as_deref()),
        },
        Some(Command::Repl) => cmd_repl(),
        Some(Command::Doctor) => cmd_doctor(),
        None => {
            println!("corvid — the AI-native language compiler");
            println!("Run `corvid --help` for usage.");
            Ok(0)
        }
    };

    match result {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(2)
        }
    }
}

// ------------------------------------------------------------
// Commands
// ------------------------------------------------------------

fn cmd_new(name: &str) -> Result<u8> {
    let root = scaffold_new(name).context("failed to scaffold project")?;
    println!("created new Corvid project at `{}`", root.display());
    println!("\nNext steps:");
    println!("  cd {name}");
    println!("  pip install corvid-runtime");
    println!("  corvid run src/main.cor");
    Ok(0)
}

fn cmd_check(file: &Path) -> Result<u8> {
    let source = std::fs::read_to_string(file)
        .with_context(|| format!("cannot read `{}`", file.display()))?;
    let config = load_corvid_config_for(file);
    let result = compile_with_config(&source, config.as_ref());
    if result.ok() {
        println!("ok: {} — no errors", file.display());
        Ok(0)
    } else {
        eprint!("{}", render_all_pretty(&result.diagnostics, file, &source));
        Ok(1)
    }
}

fn cmd_build(file: &Path, target: &str) -> Result<u8> {
    match target {
        "python" | "py" => {
            let out = build_to_disk(file)
                .with_context(|| format!("failed to build `{}`", file.display()))?;
            if let Some(path) = out.output_path {
                println!("built: {} -> {}", file.display(), path.display());
                Ok(0)
            } else {
                eprint!("{}", render_all_pretty(&out.diagnostics, file, &out.source));
                Ok(1)
            }
        }
        "native" => {
            let out = build_native_to_disk(file)
                .with_context(|| format!("failed to build `{}` (native)", file.display()))?;
            if let Some(path) = out.output_path {
                println!("built: {} -> {}", file.display(), path.display());
                Ok(0)
            } else {
                eprint!("{}", render_all_pretty(&out.diagnostics, file, &out.source));
                Ok(1)
            }
        }
        other => {
            anyhow::bail!(
                "unknown target `{other}`; valid: `python` (default), `native`"
            )
        }
    }
}

fn cmd_run(file: &Path, target: &str, tools_lib: Option<&Path>) -> Result<u8> {
    let rt = match target {
        "auto" => RunTarget::Auto,
        "native" => RunTarget::Native,
        "interp" | "interpreter" => RunTarget::Interpreter,
        other => anyhow::bail!(
            "unknown target `{other}`; valid: `auto` (default), `native`, `interpreter`"
        ),
    };
    if let Some(lib) = tools_lib {
        if !lib.exists() {
            anyhow::bail!(
                "--with-tools-lib `{}` does not exist — build the tools crate first (`cargo build -p <your-tools-crate> --release`)",
                lib.display()
            );
        }
    }
    // Auto: native AOT tier when the IR is tool-free and uses only
    // supported command-line boundary types, or when tool-using code
    // has a companion tools staticlib provided.
    // Interpreter otherwise (with a stderr notice). Native-required
    // and interpreter-forced are the explicit overrides. See
    // `RunTarget` docs in corvid-driver for the exact semantics.
    run_with_target(file, rt, tools_lib)
        .with_context(|| format!("failed to run `{}`", file.display()))
}

fn cmd_repl() -> Result<u8> {
    corvid_repl::Repl::run_stdio().context("failed to run `corvid repl`")?;
    Ok(0)
}

fn cmd_verify(corpus: Option<&Path>, shrink: Option<&Path>, json: bool) -> Result<u8> {
    match (corpus, shrink) {
        (Some(dir), None) => {
            let reports = verify_corpus(dir)?;
            let divergent: Vec<_> = reports
                .iter()
                .filter(|report| !report.divergences.is_empty())
                .collect();
            println!("{}", render_corpus_grid(&reports));
            if !divergent.is_empty() {
                println!();
                for (index, report) in divergent.iter().enumerate() {
                    if index > 0 {
                        println!();
                    }
                    println!("{}", render_report(report));
                }
            }
            if json {
                eprintln!("{}", serde_json::to_string_pretty(&reports)?);
            }
            Ok(if divergent.is_empty() { 0 } else { 1 })
        }
        (None, Some(file)) => {
            let result = shrink_program(file)?;
            println!(
                "shrunk reproducer: {} -> {} (removed {} line(s))",
                result.original.display(),
                result.output.display(),
                result.removed_lines
            );
            if json {
                eprintln!("{}", serde_json::to_string_pretty(&result)?);
            }
            Ok(0)
        }
        (None, None) => anyhow::bail!(
            "use `corvid verify --corpus <dir>` or `corvid verify --shrink <file>`"
        ),
        (Some(_), Some(_)) => unreachable!("clap enforces conflicts"),
    }
}

// ------------------------------------------------------------
// Verification suites — effect-system spec, custom dimensions,
// adversarial bypass generation
// ------------------------------------------------------------

fn cmd_test(
    target: Option<&str>,
    meta: bool,
    count: u32,
    model: &str,
) -> Result<u8> {
    match target {
        None => {
            eprintln!("`corvid test` with no target is the legacy placeholder.");
            eprintln!("Use one of: `corvid test dimensions`, `corvid test spec`,");
            eprintln!("`corvid test spec --meta`, `corvid test adversarial --count <N>`.");
            Ok(0)
        }
        Some("dimensions") => cmd_test_dimensions(),
        Some("spec") if meta => cmd_test_spec_meta(),
        Some("spec") => cmd_test_spec(),
        Some("adversarial") => cmd_test_adversarial(count, model),
        Some(other) => {
            anyhow::bail!(
                "unknown test target `{other}`; valid: `dimensions`, `spec`, `spec --meta`, `adversarial`"
            )
        }
    }
}

fn cmd_test_dimensions() -> Result<u8> {
    println!("corvid test dimensions — archetype law-check suite");
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let config = corvid_driver::load_corvid_config_for(&cwd.join("anywhere.cor"));
    match config.as_ref() {
        Some(cfg) => {
            let n = cfg.effect_system.dimensions.len();
            println!(
                "reading corvid.toml — {n} custom dimension{}",
                if n == 1 { "" } else { "s" }
            );
        }
        None => println!("no corvid.toml found — running law checks on built-ins only"),
    }
    println!("running {DEFAULT_SAMPLES} cases per law…");
    let results = run_law_checks(config.as_ref(), DEFAULT_SAMPLES);
    print!("{}", render_law_check_report(&results));
    let failures = results
        .iter()
        .filter(|r| matches!(
            r.verdict,
            corvid_driver::LawVerdict::CounterExample { .. }
        ))
        .count();
    Ok(if failures == 0 { 0 } else { 1 })
}

fn cmd_test_spec() -> Result<u8> {
    let spec_dir = PathBuf::from("docs/effects-spec");
    if !spec_dir.exists() {
        anyhow::bail!(
            "`docs/effects-spec/` not found; run `corvid test spec` from the repository root"
        );
    }
    println!("corvid test spec — verify every fenced corvid block in {}\n", spec_dir.display());
    let verdicts = verify_spec_examples(&spec_dir)
        .with_context(|| format!("failed to verify `{}`", spec_dir.display()))?;
    print!("{}", render_spec_report(&verdicts));
    let failed = verdicts
        .iter()
        .filter(|v| matches!(v.kind, VerdictKind::Fail { .. }))
        .count();
    Ok(if failed == 0 { 0 } else { 1 })
}

fn cmd_test_spec_meta() -> Result<u8> {
    println!("corvid test spec --meta — self-verifying verification\n");
    let corpus_dir = PathBuf::from("docs/effects-spec/counterexamples/composition");
    if !corpus_dir.exists() {
        anyhow::bail!(
            "counter-example corpus not found at `{}`; run from the repository root",
            corpus_dir.display()
        );
    }
    let verdicts = corvid_driver::verify_counterexample_corpus(&corpus_dir)
        .context("failed to run meta-verification harness")?;
    print!("{}", corvid_driver::render_meta_report(&verdicts));
    let failed = verdicts
        .iter()
        .filter(|v| !matches!(v.kind, corvid_driver::MetaKind::Distinguishes))
        .count();
    Ok(if failed == 0 { 0 } else { 1 })
}

fn cmd_test_adversarial(count: u32, model: &str) -> Result<u8> {
    println!("corvid test adversarial --count {count} --model {model}\n");
    println!("Drives an LLM to generate programs designed to bypass the dimensional");
    println!("effect checker. Every generated program runs through `corvid check`;");
    println!("any that compiles is a real bypass and is filed as an issue.");
    println!();
    println!("Implementation tracked in ROADMAP Phase 20g — generator not yet wired.");
    println!("See docs/effects-spec/README.md for the verification guarantees.");
    Ok(0)
}

// ------------------------------------------------------------
// Effect-diff tool
// ------------------------------------------------------------

fn cmd_effect_diff(before: &str, after: &str) -> Result<u8> {
    let before_path = PathBuf::from(before);
    let after_path = PathBuf::from(after);
    println!(
        "corvid effect-diff {} -> {}\n",
        before_path.display(),
        after_path.display(),
    );
    let before_snap = snapshot_revision(&before_path)
        .with_context(|| format!("failed to snapshot `{}`", before_path.display()))?;
    let after_snap = snapshot_revision(&after_path)
        .with_context(|| format!("failed to snapshot `{}`", after_path.display()))?;
    let diff = diff_snapshots(&before_snap, &after_snap);
    print!("{}", render_effect_diff(&diff));
    // Exit 1 when the diff is non-empty so CI can gate on
    // "unexpected effect-shape drift" if the user wants it.
    let any_change =
        !diff.added.is_empty() || !diff.removed.is_empty() || !diff.changed.is_empty();
    Ok(if any_change { 1 } else { 0 })
}

// ------------------------------------------------------------
// Dimension registry client
// ------------------------------------------------------------

fn cmd_add_dimension(spec: &str) -> Result<u8> {
    let project_dir =
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    println!("corvid add-dimension {spec}\n");
    let outcome = corvid_driver::install_dimension(spec, &project_dir)?;
    match outcome {
        corvid_driver::AddDimensionOutcome::Added { name, target } => {
            println!(
                "installed `{name}` into {}",
                target.display()
            );
            println!("run `corvid test dimensions` to re-verify every dimension.");
            Ok(0)
        }
        corvid_driver::AddDimensionOutcome::Rejected { reason } => {
            eprintln!("rejected: {reason}");
            Ok(1)
        }
    }
}

fn cmd_routing_report(
    trace_dir: Option<&Path>,
    since: Option<&str>,
    since_commit: Option<&str>,
    json: bool,
) -> Result<u8> {
    let trace_dir = trace_dir.unwrap_or_else(|| Path::new("target/trace"));
    let report = build_report(RoutingReportOptions {
        trace_dir,
        since,
        since_commit,
    })?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", render_routing_report(&report));
    }
    Ok(if report.healthy { 0 } else { 1 })
}

// ------------------------------------------------------------
// corvid doctor
// ------------------------------------------------------------

fn cmd_doctor() -> Result<u8> {
    use corvid_driver::load_dotenv_walking;

    println!("corvid doctor — checking local environment...\n");

    // Try loading .env first so the rest of the checks see what programs would.
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match load_dotenv_walking(&cwd) {
        Some(p) => println!("  ✓ .env loaded from {}", p.display()),
        None => println!("  · no .env file found from cwd upward (optional)"),
    }

    // CORVID_MODEL
    let model = std::env::var("CORVID_MODEL").ok();
    match &model {
        Some(v) => println!("  ✓ CORVID_MODEL = {v}"),
        None => println!(
            "  · CORVID_MODEL not set. Set one (e.g. `export CORVID_MODEL=gpt-4o-mini` or\n    `claude-opus-4-6`) or put `default_model = \"...\"` in corvid.toml under [llm]."
        ),
    }

    // Anthropic
    if std::env::var("ANTHROPIC_API_KEY").is_ok() {
        println!("  ✓ ANTHROPIC_API_KEY set (Claude models available)");
    } else {
        println!("  · ANTHROPIC_API_KEY not set — Claude calls will error at the prompt site");
    }

    // OpenAI
    if std::env::var("OPENAI_API_KEY").is_ok() {
        println!("  ✓ OPENAI_API_KEY set (GPT / o-series models available)");
    } else {
        println!("  · OPENAI_API_KEY not set — OpenAI calls will error at the prompt site");
    }

    // Cross-check: model prefix vs. available keys.
    if let Some(m) = &model {
        if m.starts_with("claude-") && std::env::var("ANTHROPIC_API_KEY").is_err() {
            println!(
                "  ✗ CORVID_MODEL is `{m}` but ANTHROPIC_API_KEY is not set"
            );
        }
        let openai_prefixes = ["gpt-", "o1-", "o3-", "o4-"];
        if openai_prefixes.iter().any(|p| m.starts_with(p))
            && std::env::var("OPENAI_API_KEY").is_err()
        {
            println!(
                "  ✗ CORVID_MODEL is `{m}` but OPENAI_API_KEY is not set"
            );
        }
    }

    // Python (legacy `--target=python` users only)
    let has_python = std::process::Command::new("python3")
        .arg("--version")
        .output()
        .ok()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if has_python {
        println!("  · python3 detected (legacy `--target=python` available)");
    } else {
        println!("  · python3 not detected (only needed for `--target=python`)");
    }

    println!();
    println!("native `corvid run` works without Python. Configure CORVID_MODEL + the");
    println!("matching API key and prompt-only programs run end-to-end.");
    Ok(0)
}
