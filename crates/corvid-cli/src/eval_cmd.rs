//! `corvid eval` source-level evals and retrospective model migration tooling.
//!
//! Source eval execution is owned by `corvid-driver`; this module keeps CLI
//! routing separate from the reusable runner. `--swap-model` remains the
//! Phase 20h retrospective migration mode.

use crate::{replay, test_from_traces};
use anyhow::{Context, Result};
use corvid_driver::{
    default_eval_options, load_dotenv_walking, render_eval_report, run_evals_at_path_with_options,
};
use std::path::{Path, PathBuf};

pub fn run_eval(
    inputs: &[PathBuf],
    source: Option<&Path>,
    swap_model: Option<&str>,
) -> Result<u8> {
    let Some(model) = swap_model else {
        return run_source_evals(inputs);
    };

    if inputs.is_empty() {
        anyhow::bail!("`corvid eval --swap-model` requires at least one trace file or directory");
    }

    eprintln!("eval model-swap mode - target model: `{model}`");
    let mut exit_code = 0_u8;
    for input in inputs {
        let code = if input.is_dir() {
            eprintln!("running trace-suite migration analysis: {}", input.display());
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

fn run_source_evals(inputs: &[PathBuf]) -> Result<u8> {
    if inputs.is_empty() {
        eprintln!("usage: `corvid eval <file.cor> [more.cor ...]`");
        eprintln!(
            "For model migration analysis, use `corvid eval --swap-model <MODEL> --source <FILE> <TRACE_OR_DIR>...`."
        );
        return Ok(1);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eval_without_inputs_prints_usage() {
        let code = run_eval(&[], None, None).expect("usage returns an exit code");
        assert_eq!(code, 1);
    }

    #[test]
    fn eval_swap_model_requires_inputs() {
        let err = run_eval(&[], None, Some("candidate")).unwrap_err();
        assert!(
            err.to_string().contains("requires at least one trace"),
            "{err:#}"
        );
    }
}
