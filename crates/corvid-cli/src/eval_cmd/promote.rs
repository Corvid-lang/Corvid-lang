use anyhow::{Context, Result};
use corvid_runtime::{
    promote_lineage_events_to_eval, LineageEvent, LineageRedactionPolicy,
    LINEAGE_EVAL_FIXTURE_SCHEMA,
};
use std::fs;
use std::path::{Path, PathBuf};

pub(super) fn run_promote_lineage(inputs: &[PathBuf], out_dir: Option<&Path>) -> Result<u8> {
    if inputs.is_empty() {
        eprintln!(
            "usage: `corvid eval promote <trace.lineage.jsonl> [more...] [--promote-out DIR]`"
        );
        return Ok(1);
    }
    let out_dir = out_dir.unwrap_or_else(|| Path::new("target/eval/lineage"));
    fs::create_dir_all(out_dir)
        .with_context(|| format!("creating eval fixture directory `{}`", out_dir.display()))?;
    let policy = LineageRedactionPolicy::production_default();
    for input in inputs {
        let events = read_lineage_events(input)
            .with_context(|| format!("reading lineage trace `{}`", input.display()))?;
        let fixture = promote_lineage_events_to_eval(&events, &policy)
            .with_context(|| format!("promoting lineage trace `{}`", input.display()))?;
        let file_name = format!(
            "{}.lineage-eval.json",
            sanitize_file_stem(&fixture.trace_id)
        );
        let out_path = out_dir.join(file_name);
        let json = serde_json::to_string_pretty(&fixture)
            .context("serializing promoted lineage eval fixture")?;
        fs::write(&out_path, format!("{json}\n"))
            .with_context(|| format!("writing eval fixture `{}`", out_path.display()))?;
        println!(
            "promoted: {} -> {} ({}, events={}, fixture_hash={})",
            input.display(),
            out_path.display(),
            LINEAGE_EVAL_FIXTURE_SCHEMA,
            fixture.events.len(),
            fixture.fixture_hash
        );
    }
    Ok(0)
}

fn read_lineage_events(path: &Path) -> Result<Vec<LineageEvent>> {
    let text = fs::read_to_string(path)?;
    let mut events = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let event: LineageEvent = serde_json::from_str(trimmed)
            .with_context(|| format!("line {} is not a lineage event", index + 1))?;
        events.push(event);
    }
    Ok(events)
}

fn sanitize_file_stem(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "trace".to_string()
    } else {
        sanitized
    }
}

pub(super) fn latest_summary_path_for_source(source: &Path) -> PathBuf {
    let base = source.parent().unwrap_or_else(|| Path::new("."));
    let stem = source
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("suite");
    base.join("target")
        .join("eval")
        .join(sanitize_path_segment(stem))
        .join("latest.json")
}

fn sanitize_path_segment(raw: &str) -> String {
    let sanitized: String = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "suite".into()
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::super::run_eval;
    use super::*;

    #[test]
    fn eval_promote_writes_redacted_lineage_fixture() {
        let dir = tempfile::tempdir().expect("tempdir");
        let trace_path = dir.path().join("trace-1.lineage.jsonl");
        let out_dir = dir.path().join("fixtures");
        let mut route = corvid_runtime::LineageEvent::root(
            "trace-1",
            corvid_runtime::LineageKind::Route,
            "POST /send",
            1,
        )
        .finish(corvid_runtime::LineageStatus::Ok, 10);
        route.replay_key = "replay-secret".to_string();
        let mut tool = corvid_runtime::LineageEvent::child(
            &route,
            corvid_runtime::LineageKind::Tool,
            "email alice@example.com",
            0,
            2,
        )
        .finish(corvid_runtime::LineageStatus::Failed, 8);
        tool.guarantee_id = "approval.reachable_entrypoints_require_contract".to_string();
        tool.effect_ids = vec!["send_email".to_string()];
        let body = [route, tool]
            .iter()
            .map(|event| serde_json::to_string(event).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&trace_path, format!("{body}\n")).expect("trace");

        let code = run_eval(
            &[PathBuf::from("promote"), trace_path.clone()],
            None,
            None,
            None,
            None,
            Some(&out_dir),
        )
        .expect("promote");
        assert_eq!(code, 0);
        let fixture_path = out_dir.join("trace-1.lineage-eval.json");
        let json = std::fs::read_to_string(fixture_path).expect("fixture");
        assert!(json.contains(LINEAGE_EVAL_FIXTURE_SCHEMA));
        assert!(json.contains("fixture_hash"));
        assert!(!json.contains("alice@example.com"));
        assert!(!json.contains("replay-secret"));
    }
}
