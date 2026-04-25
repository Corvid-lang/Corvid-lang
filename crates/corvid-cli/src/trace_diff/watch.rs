//! Reactive local trace-diff mode.
//!
//! `--format=watch` is not another serialized receipt shape. It is
//! a development loop: compare the base commit against the current
//! working-tree file, render once, then rerender whenever that file
//! changes. The implementation intentionally uses std polling
//! instead of adding a platform watcher dependency; correctness here
//! is "never miss a stable file-content change", not nanosecond
//! filesystem event fidelity.

use std::fs;
use std::path::Path;
use std::thread;
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, Context, Result};
use corvid_driver::{compile_to_abi_with_config, load_corvid_config_for};
use sha2::Digest;

use super::impact::{compute_trace_impact, TraceImpact};
use super::narrative::compute_diff_summary;
use super::policy;
use super::receipt::Receipt;
use super::reviewer_invocation::invoke_reviewer;
use super::{git_show, resolve_narrative, TraceDiffArgs};

pub(super) fn run_watch(args: &TraceDiffArgs<'_>) -> Result<u8> {
    if args.stack_spec.is_some() {
        return Err(anyhow!(
            "`--format=watch` watches a single working-tree file; combine stack review with normal `--format=json`"
        ));
    }
    if args.sign_key_path.is_some() || std::env::var_os("CORVID_SIGNING_KEY").is_some() {
        return Err(anyhow!(
            "`--format=watch` is an interactive terminal mode and cannot be signed; use `--format=json --sign` for CI artifacts"
        ));
    }

    let poll = watch_poll_interval();
    let render_limit = watch_render_limit();
    let mut last = None;
    let mut renders = 0usize;
    loop {
        let fingerprint = source_fingerprint(args.source_path)?;
        if last.as_ref() != Some(&fingerprint) {
            last = Some(fingerprint);
            renders += 1;
            println!(
                "\n=== corvid trace-diff watch render #{renders}: {} ===",
                args.source_path.display()
            );
            match render_working_tree_receipt(args) {
                Ok(rendered) => print!("{rendered}"),
                Err(e) => eprintln!("watch render failed: {e:#}"),
            }
            if render_limit.is_some_and(|limit| renders >= limit) {
                return Ok(0);
            }
        }
        thread::sleep(poll);
    }
}

fn render_working_tree_receipt(args: &TraceDiffArgs<'_>) -> Result<String> {
    let base_source = git_show(args.base_sha, args.source_path).with_context(|| {
        format!(
            "fetching `{}` at base `{}`",
            args.source_path.display(),
            args.base_sha
        )
    })?;
    let head_source = fs::read_to_string(args.source_path).with_context(|| {
        format!(
            "reading working-tree `{}` for watch render",
            args.source_path.display()
        )
    })?;

    let config = load_corvid_config_for(args.source_path);
    let source_path_str = args.source_path.to_string_lossy().replace('\\', "/");
    let generated_at = "1970-01-01T00:00:00Z";
    let base_abi = compile_to_abi_with_config(
        &base_source,
        &source_path_str,
        generated_at,
        config.as_ref(),
    )
    .map_err(|diags| {
        anyhow!(
            "base source at `{}` failed to compile: {} diagnostic(s)",
            args.base_sha,
            diags.len()
        )
    })?;
    let head_abi = compile_to_abi_with_config(
        &head_source,
        &source_path_str,
        generated_at,
        config.as_ref(),
    )
    .map_err(|diags| {
        anyhow!(
            "working-tree source failed to compile: {} diagnostic(s)",
            diags.len()
        )
    })?;

    let impact = match args.trace_dir {
        Some(dir) => compute_trace_impact(&base_source, &head_source, args.source_path, dir)
            .context("counterfactual replay failed")?,
        None => TraceImpact::empty(),
    };
    let diff = compute_diff_summary(&base_abi, &head_abi);
    let (narrative, narrative_rejected) = resolve_narrative(&diff, args.narrative_mode)?;
    let receipt = Receipt::build(
        args.base_sha,
        "working-tree",
        &source_path_str,
        &base_abi,
        &head_abi,
        impact,
        narrative.clone(),
        narrative_rejected,
    );
    let verdict = policy::apply_policy(&receipt, args.policy_path)?;
    let mut rendered = invoke_reviewer(&base_abi, &head_abi, &receipt.impact, &narrative)
        .context("reviewer agent execution failed")?;
    rendered.push_str("\n---\n");
    rendered.push_str(if verdict.ok {
        "watch verdict: ok\n"
    } else {
        "watch verdict: regression policy tripped\n"
    });
    for flag in &verdict.flags {
        rendered.push_str("- ");
        rendered.push_str(flag);
        rendered.push('\n');
    }
    Ok(rendered)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceFingerprint {
    modified: Option<SystemTime>,
    len: u64,
    hash: [u8; 32],
}

fn source_fingerprint(path: &Path) -> Result<SourceFingerprint> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("reading metadata for watched file `{}`", path.display()))?;
    let bytes = fs::read(path)
        .with_context(|| format!("reading watched file `{}`", path.display()))?;
    Ok(SourceFingerprint {
        modified: metadata.modified().ok(),
        len: metadata.len(),
        hash: sha2::Sha256::digest(&bytes).into(),
    })
}

fn watch_poll_interval() -> Duration {
    let millis = std::env::var("CORVID_TRACE_DIFF_WATCH_POLL_MS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(500);
    Duration::from_millis(millis)
}

fn watch_render_limit() -> Option<usize> {
    std::env::var("CORVID_TRACE_DIFF_WATCH_LIMIT")
        .ok()
        .and_then(|raw| raw.parse::<usize>().ok())
        .filter(|n| *n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_poll_interval_has_safe_default() {
        assert_eq!(watch_poll_interval(), Duration::from_millis(500));
    }
}
