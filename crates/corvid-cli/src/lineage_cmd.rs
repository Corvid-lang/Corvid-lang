use anyhow::{Context, Result};
use corvid_runtime::{render_lineage_tree, LineageEvent};
use std::fs;
use std::path::{Path, PathBuf};

pub fn run_lineage(id_or_path: &str, trace_dir: Option<&Path>) -> Result<u8> {
    let path = resolve_lineage_path(id_or_path, trace_dir);
    let events = read_lineage_events(&path)
        .with_context(|| format!("reading lineage trace `{}`", path.display()))?;
    print!("{}", render_lineage_tree(&events));
    Ok(0)
}

fn resolve_lineage_path(id_or_path: &str, trace_dir: Option<&Path>) -> PathBuf {
    let direct = PathBuf::from(id_or_path);
    if direct.exists() || direct.extension().is_some() {
        return direct;
    }
    trace_dir
        .unwrap_or_else(|| Path::new("target/trace"))
        .join(format!("{id_or_path}.lineage.jsonl"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use corvid_runtime::{LineageEvent, LineageKind, LineageStatus};

    #[test]
    fn reads_lineage_jsonl_from_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.lineage.jsonl");
        let route = LineageEvent::root("trace-1", LineageKind::Route, "GET /", 1)
            .finish(LineageStatus::Ok, 2);
        fs::write(
            &path,
            format!("{}\n", serde_json::to_string(&route).unwrap()),
        )
        .unwrap();
        let events = read_lineage_events(&path).unwrap();
        assert_eq!(events, vec![route]);
    }

    #[test]
    fn bare_id_resolves_to_lineage_jsonl_under_trace_dir() {
        let dir = tempfile::tempdir().unwrap();
        let path = resolve_lineage_path("trace-1", Some(dir.path()));
        assert_eq!(path, dir.path().join("trace-1.lineage.jsonl"));
    }
}
