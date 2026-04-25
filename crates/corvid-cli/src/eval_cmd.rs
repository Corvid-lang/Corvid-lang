//! `corvid eval --swap-model` retrospective model migration tooling.
//!
//! Full source-level `eval` execution is Phase 27. This module ships the
//! Phase 20h model-substrate piece: replay existing traces against a candidate
//! model and report divergence using the same engine as `corvid replay --model`.

use crate::{replay, test_from_traces};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub fn run_eval(
    inputs: &[PathBuf],
    source: Option<&Path>,
    swap_model: Option<&str>,
) -> Result<u8> {
    let Some(model) = swap_model else {
        eprintln!(
            "note: `corvid eval` source-level eval execution ships in Phase 27. \
             Today this command supports retrospective migration via \
             `corvid eval --swap-model <MODEL> --source <FILE> <TRACE_OR_DIR>...`."
        );
        return Ok(1);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eval_without_swap_model_is_explicit_phase_27_stub() {
        let code = run_eval(&[], None, None).expect("stub returns an exit code");
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
