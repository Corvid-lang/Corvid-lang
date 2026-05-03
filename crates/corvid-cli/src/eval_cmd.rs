//! `corvid eval` source-level evals and retrospective model migration tooling.
//!
//! Source eval execution is owned by `corvid-driver`; this module keeps CLI
//! routing separate from the reusable runner. `--swap-model` remains the
//! Phase 20h retrospective migration mode.

use crate::{replay, test_from_traces};
use anyhow::{Context, Result};
use compare::{read_summary_cost_usd, run_compare};
use corvid_driver::{
    default_eval_options, load_dotenv_walking, render_eval_report, run_evals_at_path_with_options,
};
use promote::{latest_summary_path_for_source, run_promote_lineage};
use std::path::{Path, PathBuf};

mod compare;
mod promote;

pub fn run_eval(
    inputs: &[PathBuf],
    source: Option<&Path>,
    swap_model: Option<&str>,
    max_spend: Option<f64>,
    golden_traces: Option<&Path>,
    promote_out: Option<&Path>,
) -> Result<u8> {
    if golden_traces.is_some() && swap_model.is_some() {
        anyhow::bail!("`corvid eval --golden-traces` and `--swap-model` are separate modes");
    }
    if let Some(trace_dir) = golden_traces {
        return run_golden_trace_evals(inputs, source, trace_dir);
    }
    let Some(model) = swap_model else {
        return run_source_evals(inputs, max_spend, promote_out);
    };

    if inputs.is_empty() {
        anyhow::bail!("`corvid eval --swap-model` requires at least one trace file or directory");
    }

    eprintln!("eval model-swap mode - target model: `{model}`");
    let mut exit_code = 0_u8;
    for input in inputs {
        let code = if input.is_dir() {
            eprintln!(
                "running trace-suite migration analysis: {}",
                input.display()
            );
            test_from_traces::run_test_from_traces(test_from_traces::TestFromTracesArgs {
                trace_dir: input,
                source,
                replay_model: Some(model),
                only_dangerous: false,
                only_prompt: None,
                only_tool: None,
                since: None,
                promote: false,
                flake_detect: None,
            })
            .with_context(|| format!("failed to evaluate trace directory `{}`", input.display()))?
        } else {
            eprintln!("running trace migration analysis: {}", input.display());
            replay::run_replay(input, source, Some(model), None)
                .with_context(|| format!("failed to evaluate trace `{}`", input.display()))?
        };
        exit_code = exit_code.max(code);
    }

    Ok(exit_code)
}

fn run_golden_trace_evals(
    inputs: &[PathBuf],
    source: Option<&Path>,
    trace_dir: &Path,
) -> Result<u8> {
    let mut sources = inputs.to_vec();
    if sources.is_empty() {
        if let Some(source) = source {
            sources.push(source.to_path_buf());
        }
    }
    if sources.is_empty() {
        eprintln!("usage: `corvid eval --golden-traces <DIR> <source.cor>`");
        return Ok(1);
    }

    let mut exit_code = 0_u8;
    for source in &sources {
        eprintln!(
            "golden-trace eval: source `{}` against `{}`",
            source.display(),
            trace_dir.display()
        );
        let code = test_from_traces::run_test_from_traces(test_from_traces::TestFromTracesArgs {
            trace_dir,
            source: Some(source.as_path()),
            replay_model: None,
            only_dangerous: false,
            only_prompt: None,
            only_tool: None,
            since: None,
            promote: false,
            flake_detect: None,
        })
        .with_context(|| {
            format!(
                "failed golden-trace eval for `{}` against `{}`",
                source.display(),
                trace_dir.display()
            )
        })?;
        exit_code = exit_code.max(code);
    }
    Ok(exit_code)
}

fn run_source_evals(
    inputs: &[PathBuf],
    max_spend: Option<f64>,
    promote_out: Option<&Path>,
) -> Result<u8> {
    if inputs
        .first()
        .and_then(|input| input.to_str())
        .is_some_and(|input| input == "compare")
    {
        if promote_out.is_some() {
            anyhow::bail!("`corvid eval compare` does not accept `--promote-out`");
        }
        return run_compare(&inputs[1..]);
    }
    if inputs
        .first()
        .and_then(|input| input.to_str())
        .is_some_and(|input| input == "promote")
    {
        return run_promote_lineage(&inputs[1..], promote_out);
    }
    if promote_out.is_some() {
        anyhow::bail!("`--promote-out` is only valid with `corvid eval promote <trace>`");
    }

    if inputs.is_empty() {
        eprintln!("usage: `corvid eval <file.cor> [more.cor ...]`");
        eprintln!("       `corvid eval compare <base>..<head>`");
        eprintln!("       `corvid eval promote <trace.lineage.jsonl> [--promote-out DIR]`");
        eprintln!(
            "For model migration analysis, use `corvid eval --swap-model <MODEL> --source <FILE> <TRACE_OR_DIR>...`."
        );
        return Ok(1);
    }

    if let Some(max_spend) = configured_max_spend(max_spend)? {
        if !max_spend.is_finite() || max_spend < 0.0 {
            anyhow::bail!("eval budget must be a finite non-negative USD amount");
        }
        let planned = planned_eval_spend(inputs)?;
        if planned > max_spend {
            eprintln!(
                "eval budget exceeded before running: planned ${planned:.6} > max ${max_spend:.6}"
            );
            return Ok(1);
        }
        eprintln!("eval budget: planned ${planned:.6} <= max ${max_spend:.6}");
    }

    let mut exit_code = 0_u8;
    for input in inputs {
        let dotenv_start = input.parent().unwrap_or_else(|| Path::new("."));
        load_dotenv_walking(dotenv_start);
        let runtime = corvid_driver::Runtime::builder().build();
        let source = std::fs::read_to_string(input).ok();
        let tokio = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("failed to initialize async eval runtime")?;
        let report = tokio
            .block_on(run_evals_at_path_with_options(
                input,
                &runtime,
                default_eval_options(input),
            ))
            .map_err(anyhow::Error::new)?;
        print!("{}", render_eval_report(&report, source.as_deref()));
        exit_code = exit_code.max(report.exit_code());
    }
    Ok(exit_code)
}

fn configured_max_spend(cli: Option<f64>) -> Result<Option<f64>> {
    if cli.is_some() {
        return Ok(cli);
    }
    match std::env::var("CORVID_EVAL_MAX_SPEND_USD") {
        Ok(raw) => raw
            .parse::<f64>()
            .map(Some)
            .with_context(|| "CORVID_EVAL_MAX_SPEND_USD must be a number"),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(error).context("failed to read CORVID_EVAL_MAX_SPEND_USD"),
    }
}

fn planned_eval_spend(inputs: &[PathBuf]) -> Result<f64> {
    inputs.iter().try_fold(0.0, |total, input| {
        Ok(total
            + prior_eval_cost(input).with_context(|| {
                format!("failed to estimate eval spend for `{}`", input.display())
            })?)
    })
}

fn prior_eval_cost(source: &Path) -> Result<f64> {
    let summary_path = latest_summary_path_for_source(source);
    if !summary_path.exists() {
        return Ok(0.0);
    }
    read_summary_cost_usd(&summary_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eval_without_inputs_prints_usage() {
        let code = run_eval(&[], None, None, None, None, None).expect("usage returns an exit code");
        assert_eq!(code, 1);
    }

    #[test]
    fn eval_swap_model_requires_inputs() {
        let err = run_eval(&[], None, Some("candidate"), None, None, None).unwrap_err();
        assert!(
            err.to_string().contains("requires at least one trace"),
            "{err:#}"
        );
    }

    #[test]
    fn eval_budget_fails_before_running_when_prior_cost_exceeds_max() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source = dir.path().join("suite.cor");
        std::fs::write(&source, "eval math:\n    assert true\n").expect("source");
        let summary_path = latest_summary_path_for_source(&source);
        std::fs::create_dir_all(summary_path.parent().unwrap()).expect("summary dir");
        std::fs::write(
            &summary_path,
            r#"{
  "source_path": "suite.cor",
  "evals": [],
  "compile_ok": true,
  "trace": { "total_cost_usd": 0.25, "total_latency_ms": 0, "prompts": [], "model_routes": [] }
}"#,
        )
        .expect("summary");

        let code = run_eval(&[source], None, None, Some(0.10), None, None).expect("budget result");
        assert_eq!(code, 1);
    }

    #[test]
    fn eval_golden_traces_and_swap_model_are_exclusive() {
        let err = run_eval(
            &[],
            None,
            Some("candidate"),
            None,
            Some(Path::new("traces")),
            None,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("separate modes"),
            "unexpected error: {err:#}"
        );
    }
}
