//! `corvid test` CLI dispatch — slices 8 / 12 / 22 testing
//! surface, decomposed in Phase 20j-A1.
//!
//! Houses every `cmd_test_*` dispatch arm:
//!
//! - [`cmd_test`] runs the per-decl test suites (or
//!   `--target <path>` for a single corpus).
//! - [`cmd_test_file`] runs the tests in one source file.
//! - [`cmd_test_dimensions`] runs the custom-dimension
//!   verification suite.
//! - [`cmd_test_spec`] / [`cmd_test_spec_site`] /
//!   [`cmd_test_spec_meta`] verify the effect-system spec
//!   examples and emit the spec site / meta-spec reports.
//! - [`cmd_test_rewrites`] runs the law-rewrite checks.
//! - [`cmd_test_adversarial`] runs the adversarial-bypass
//!   corpus and files GitHub issues for any escapes.
//!
//! All actual work happens in `corvid_driver`; this module
//! owns only the CLI shape + report rendering.

use crate::cost_frontier::{
    build_frontier, render_frontier as render_cost_frontier, CostFrontierOptions,
};
use anyhow::{Context, Result};
use corvid_driver::{
    build_spec_site, file_github_issues_for_escapes, inspect_import_semantics,
    load_corvid_config_with_path_for, load_dotenv_walking, render_adversarial_report,
    render_dimension_verification_report, render_import_semantic_summaries, render_spec_report,
    render_spec_site_report, render_test_report, run_adversarial_suite, run_dimension_verification,
    run_tests_at_path_with_options, test_options, verify_spec_examples, AnthropicAdapter,
    EnvVarMockAdapter, OllamaAdapter, OpenAiAdapter, Runtime, StdinApprover, Tracer, VerdictKind,
    DEFAULT_SAMPLES,
};
use std::path::{Path, PathBuf};

pub(crate) fn cmd_test(
    target: Option<&str>,
    meta: bool,
    site_out: Option<&Path>,
    count: u32,
    model: &str,
    update_snapshots: bool,
) -> Result<u8> {
    match target {
        None => {
            eprintln!("usage: `corvid test <file.cor>`");
            eprintln!("Legacy verification targets are still available:");
            eprintln!("  `corvid test dimensions`, `corvid test spec`,");
            eprintln!(
                "  `corvid test spec --meta`, `corvid test rewrites`, `corvid test adversarial --count <N>`."
            );
            Ok(1)
        }
        Some("dimensions") => cmd_test_dimensions(),
        Some("spec") if site_out.is_some() => cmd_test_spec_site(site_out.unwrap()),
        Some("spec") if meta => cmd_test_spec_meta(),
        Some("spec") => cmd_test_spec(),
        Some("rewrites") => cmd_test_rewrites(),
        Some("adversarial") => cmd_test_adversarial(count, model),
        Some(other) if other.ends_with(".cor") || Path::new(other).exists() => {
            cmd_test_file(Path::new(other), update_snapshots)
        }
        Some(other) => {
            anyhow::bail!(
                "unknown test target `{other}`; pass a `.cor` file or one of: `dimensions`, `spec`, `spec --meta`, `rewrites`, `adversarial`"
            )
        }
    }
}

pub(crate) fn cmd_test_file(path: &Path, update_snapshots: bool) -> Result<u8> {
    let dotenv_start = path.parent().unwrap_or_else(|| Path::new("."));
    load_dotenv_walking(dotenv_start);
    let runtime = cli_test_runtime(path);
    let source = std::fs::read_to_string(path).ok();
    let tokio = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to initialize async test runtime")?;
    let report = tokio
        .block_on(run_tests_at_path_with_options(
            path,
            &runtime,
            test_options(path, update_snapshots),
        ))
        .map_err(anyhow::Error::new)?;
    print!("{}", render_test_report(&report, source.as_deref()));
    Ok(report.exit_code())
}

fn cli_test_runtime(path: &Path) -> Runtime {
    let trace_dir = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("target")
        .join("trace");
    let mut builder = Runtime::builder()
        .approver(std::sync::Arc::new(StdinApprover::new()))
        .tracer(Tracer::open(&trace_dir, corvid_driver::fresh_run_id()));

    if let Ok(model) = std::env::var("CORVID_MODEL") {
        builder = builder.default_model(&model);
    }
    if std::env::var("CORVID_TEST_MOCK_LLM").ok().as_deref() == Some("1") {
        builder = builder.llm(std::sync::Arc::new(EnvVarMockAdapter::from_env()));
    }
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        builder = builder.llm(std::sync::Arc::new(AnthropicAdapter::new(key)));
    }
    if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        builder = builder.llm(std::sync::Arc::new(OpenAiAdapter::new(key)));
    }
    builder
        .llm(std::sync::Arc::new(OllamaAdapter::new()))
        .build()
}

pub(crate) fn cmd_cost_frontier(
    prompt: &str,
    trace_dir: Option<&Path>,
    since: Option<&str>,
    since_commit: Option<&str>,
    json: bool,
) -> Result<u8> {
    let trace_dir = trace_dir.unwrap_or_else(|| Path::new("target/trace"));
    let report = build_frontier(CostFrontierOptions {
        prompt,
        trace_dir,
        since,
        since_commit,
    })?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", render_cost_frontier(&report));
    }
    Ok(if report.has_quality_evidence { 0 } else { 1 })
}

pub(crate) fn cmd_import_summary(file: &Path, json: bool) -> Result<u8> {
    let summaries = inspect_import_semantics(file)?;
    if json {
        let payload = summaries
            .iter()
            .map(|summary| {
                serde_json::json!({
                    "import": summary.import,
                    "path": summary.path.display().to_string(),
                    "content_hash": summary.content_hash,
                    "summary": summary.summary,
                })
            })
            .collect::<Vec<_>>();
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        print!("{}", render_import_semantic_summaries(&summaries));
    }
    Ok(0)
}

pub(crate) fn cmd_test_dimensions() -> Result<u8> {
    println!("corvid test dimensions — archetype law-check suite");
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let config_with_path = load_corvid_config_with_path_for(&cwd.join("anywhere.cor"));
    let config = config_with_path.as_ref().map(|(_, config)| config);
    match config {
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
    let config_dir = config_with_path
        .as_ref()
        .and_then(|(path, _)| path.parent());
    let report = run_dimension_verification(config, config_dir, DEFAULT_SAMPLES);
    print!("{}", render_dimension_verification_report(&report));
    let law_failures = report
        .laws
        .iter()
        .filter(|r| matches!(r.verdict, corvid_driver::LawVerdict::CounterExample { .. }))
        .count();
    let proof_failures = report.proofs.iter().filter(|p| p.failed()).count();
    Ok(if law_failures == 0 && proof_failures == 0 {
        0
    } else {
        1
    })
}

pub(crate) fn cmd_test_spec() -> Result<u8> {
    let spec_dir = PathBuf::from("docs/effects-spec");
    if !spec_dir.exists() {
        anyhow::bail!(
            "`docs/effects-spec/` not found; run `corvid test spec` from the repository root"
        );
    }
    println!(
        "corvid test spec — verify every fenced corvid block in {}\n",
        spec_dir.display()
    );
    let verdicts = verify_spec_examples(&spec_dir)
        .with_context(|| format!("failed to verify `{}`", spec_dir.display()))?;
    print!("{}", render_spec_report(&verdicts));
    let failed = verdicts
        .iter()
        .filter(|v| matches!(v.kind, VerdictKind::Fail { .. }))
        .count();
    Ok(if failed == 0 { 0 } else { 1 })
}

pub(crate) fn cmd_test_spec_site(out_dir: &Path) -> Result<u8> {
    let spec_dir = PathBuf::from("docs/effects-spec");
    if !spec_dir.exists() {
        anyhow::bail!(
            "`docs/effects-spec/` not found; run `corvid test spec --site-out <DIR>` from the repository root"
        );
    }
    println!(
        "corvid test spec --site-out {} — render executable spec site\n",
        out_dir.display()
    );
    let report = build_spec_site(&spec_dir, out_dir)
        .with_context(|| format!("failed to render spec site to `{}`", out_dir.display()))?;
    print!("{}", render_spec_site_report(&report));
    Ok(0)
}

pub(crate) fn cmd_test_spec_meta() -> Result<u8> {
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

pub(crate) fn cmd_test_rewrites() -> Result<u8> {
    println!("corvid test rewrites — preserved-semantics effect-profile fuzzing\n");
    println!("Each rewrite row names the semantic law it is obligated to preserve.");
    println!(
        "If a profile drifts, the failure report names the rewrite rule, law, line, and shrunk reproducer.\n"
    );
    let matrix = corvid_differential_verify::fuzz::build_coverage_matrix()
        .context("preserved-semantics rewrite verification failed")?;
    print!(
        "{}",
        corvid_differential_verify::fuzz::render_coverage_matrix(&matrix)
    );
    println!();
    Ok(0)
}

pub(crate) fn cmd_test_adversarial(count: u32, model: &str) -> Result<u8> {
    println!("corvid test adversarial --count {count} --model {model}\n");
    let mut report = run_adversarial_suite(count, model);
    file_github_issues_for_escapes(&mut report)?;
    print!("{}", render_adversarial_report(&report));
    Ok(if report.escaped_count == 0 { 0 } else { 1 })
}
