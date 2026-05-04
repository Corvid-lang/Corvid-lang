//! Plain replay CLI wire.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use corvid_driver::{run_replay_from_source, ReplayMode};
use corvid_trace_schema::TraceEvent;

pub(super) fn plain_replay_live(
    trace: &Path,
    source: Option<&Path>,
    events: &[TraceEvent],
) -> Result<u8> {
    let source_path = resolve_source_path(trace, source, events)?;

    eprintln!();
    eprintln!("plain replay mode");
    eprintln!("    source:   {}", source_path.display());
    eprintln!("    compiling source and substituting recorded responses...");
    eprintln!();

    let outcome = run_replay_from_source(trace, &source_path, ReplayMode::Plain)?;
    if let Some(err) = outcome.result_error {
        anyhow::bail!("plain replay failed for agent `{}`: {err}", outcome.agent_name);
    }
    if let Some(value) = outcome.result_value {
        println!("replay completed: agent `{}` -> {value}", outcome.agent_name);
    } else {
        println!("replay completed: agent `{}`", outcome.agent_name);
    }
    Ok(0)
}

fn resolve_source_path(
    trace: &Path,
    source: Option<&Path>,
    events: &[TraceEvent],
) -> Result<PathBuf> {
    if let Some(source) = source {
        return Ok(source.to_path_buf());
    }
    let recorded = events.iter().find_map(|event| match event {
        TraceEvent::SchemaHeader {
            source_path: Some(path),
            ..
        } => Some(path.as_str()),
        _ => None,
    });
    let Some(recorded) = recorded else {
        anyhow::bail!(
            "`corvid replay <trace>` needs either `--source <FILE>` or a SchemaHeader.source_path \
             recorded in the trace"
        );
    };
    let path = PathBuf::from(recorded);
    if path.is_absolute() {
        return Ok(path);
    }
    let parent = trace
        .parent()
        .with_context(|| format!("trace `{}` has no parent directory", trace.display()))?;
    Ok(parent.join(path))
}
