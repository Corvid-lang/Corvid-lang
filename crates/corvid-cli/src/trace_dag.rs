//! `corvid trace dag <trace>` — render the Grounded<T> provenance
//! DAG of a recorded trace as a Graphviz DOT graph.
//!
//! Reads every `ProvenanceEdge` event from the trace and emits a
//! directed graph with one node per unique `node_id` and one edge
//! per (parent, node) pair. Non-provenance events are ignored —
//! the renderer is a pure projection of the DAG substream onto
//! DOT syntax and does not care about the surrounding run order.
//!
//! Output goes to stdout so callers can pipe into `dot -Tsvg` or
//! any other Graphviz consumer. Traces without provenance events
//! produce an empty-but-valid `digraph provenance {}` plus a
//! warning on stderr — failing hard on "no provenance" would
//! break the intended pipeline shape.

use anyhow::{Context, Result};
use corvid_trace_schema::{read_events_from_path, TraceEvent};
use std::fmt::Write as _;
use std::path::Path;

use crate::trace_cmd::resolve_trace_path;

/// Entry for `corvid trace dag <id-or-path> [--trace-dir <path>]`.
pub fn run_dag(id_or_path: &str, trace_dir: Option<&Path>) -> Result<u8> {
    let path = resolve_trace_path(id_or_path, trace_dir)
        .with_context(|| format!("failed to locate trace `{id_or_path}`"))?;

    let events = read_events_from_path(&path)
        .with_context(|| format!("failed to read trace at `{}`", path.display()))?;

    if events.is_empty() {
        anyhow::bail!("trace `{}` is empty", path.display());
    }

    let edges = collect_provenance(&events);
    if edges.is_empty() {
        eprintln!(
            "warning: trace `{}` contains no provenance events; \
             emitting an empty DAG. Once the runtime emits \
             ProvenanceEdge at each Grounded<T> construction site, \
             this graph will populate automatically.",
            path.display()
        );
    }

    print!("{}", render_dot(&edges));
    Ok(0)
}

/// One recorded provenance edge, flattened for easy rendering.
struct ProvenanceRow<'a> {
    node_id: &'a str,
    parents: &'a [String],
    op: &'a str,
    label: Option<&'a str>,
}

fn collect_provenance(events: &[TraceEvent]) -> Vec<ProvenanceRow<'_>> {
    events
        .iter()
        .filter_map(|event| match event {
            TraceEvent::ProvenanceEdge {
                node_id,
                parents,
                op,
                label,
                ..
            } => Some(ProvenanceRow {
                node_id: node_id.as_str(),
                parents: parents.as_slice(),
                op: op.as_str(),
                label: label.as_deref(),
            }),
            _ => None,
        })
        .collect()
}

/// Render a slice of provenance rows as a DOT digraph. The result
/// includes a trailing newline so piping into `dot` works cleanly.
fn render_dot(rows: &[ProvenanceRow<'_>]) -> String {
    let mut out = String::new();
    writeln!(out, "digraph provenance {{").unwrap();
    writeln!(out, "  rankdir=LR;").unwrap();
    writeln!(out, "  node [shape=box, fontname=\"monospace\"];").unwrap();

    // Nodes first, so any parent referenced before its own row is
    // still declared. Walk every row plus every parent referenced
    // by any row.
    let mut declared: std::collections::BTreeSet<&str> =
        std::collections::BTreeSet::new();
    for row in rows {
        if declared.insert(row.node_id) {
            writeln!(out, "  {};", format_node(row.node_id, row.label, Some(row.op)))
                .unwrap();
        }
        for parent in row.parents {
            if declared.insert(parent.as_str()) {
                // Parent referenced but not itself a ProvenanceEdge
                // row — render as bare id (no op info available).
                writeln!(out, "  {};", format_node(parent, None, None)).unwrap();
            }
        }
    }

    for row in rows {
        for parent in row.parents {
            writeln!(
                out,
                "  {} -> {};",
                quote_dot(parent),
                quote_dot(row.node_id)
            )
            .unwrap();
        }
    }

    writeln!(out, "}}").unwrap();
    out
}

/// Format one node declaration: `"id" [label="..."]`. When a
/// user-supplied `label` is present it wins; otherwise the label
/// shows `node_id\l<op>` so the reader sees both the stable id
/// and the operation that produced the value.
fn format_node(node_id: &str, label: Option<&str>, op: Option<&str>) -> String {
    let rendered_label = match (label, op) {
        (Some(user), _) => user.to_string(),
        (None, Some(op)) => format!("{node_id}\\n({op})"),
        (None, None) => node_id.to_string(),
    };
    format!(
        "{} [label={}]",
        quote_dot(node_id),
        quote_dot(&rendered_label)
    )
}

/// Quote a string for DOT. DOT supports backslash-escaped double
/// quotes and backslashes within quoted ids.
fn quote_dot(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use corvid_trace_schema::{
        write_events_to_path, TraceEvent, SCHEMA_VERSION, WRITER_INTERPRETER,
    };
    use std::path::PathBuf;

    fn test_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "corvid-cli-trace-dag-test-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_with_edges(dir: &Path, run_id: &str, edges: Vec<TraceEvent>) -> PathBuf {
        let path = dir.join(format!("{run_id}.jsonl"));
        let mut events = vec![TraceEvent::SchemaHeader {
            version: SCHEMA_VERSION,
            writer: WRITER_INTERPRETER.into(),
            commit_sha: None,
            source_path: None,
            ts_ms: 0,
            run_id: run_id.into(),
        }];
        events.extend(edges);
        write_events_to_path(&path, &events).unwrap();
        path
    }

    #[allow(dead_code)]
    fn edge(
        node_id: &str,
        parents: &[&str],
        op: &str,
        label: Option<&str>,
    ) -> TraceEvent {
        TraceEvent::ProvenanceEdge {
            ts_ms: 0,
            run_id: "test".into(),
            node_id: node_id.into(),
            parents: parents.iter().map(|p| (*p).to_string()).collect(),
            op: op.into(),
            label: label.map(|s| s.to_string()),
        }
    }

    #[test]
    fn empty_trace_is_rejected() {
        let dir = test_dir();
        let path = dir.join("empty.jsonl");
        std::fs::write(&path, "").unwrap();
        let err = run_dag(path.to_str().unwrap(), None).unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn missing_trace_file_reports_clean_io_error() {
        let path = std::env::temp_dir().join(format!(
            "corvid-cli-trace-dag-missing-{}.jsonl",
            std::process::id()
        ));
        if path.exists() {
            std::fs::remove_file(&path).unwrap();
        }
        let err = run_dag(path.to_str().unwrap(), None).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("locate trace") || msg.contains("read trace"));
    }

    #[test]
    fn trace_with_no_provenance_edges_emits_empty_graph() {
        let dir = test_dir();
        let path = write_with_edges(&dir, "run-empty-prov", vec![]);
        // Smoke — run_dag prints to stdout; we just assert it
        // returns Ok. The unit-testable rendering path is through
        // render_dot directly.
        let code = run_dag(path.to_str().unwrap(), None).unwrap();
        assert_eq!(code, 0);
    }

    #[test]
    fn render_dot_produces_valid_empty_digraph() {
        let dot = render_dot(&[]);
        assert!(dot.starts_with("digraph provenance {"));
        assert!(dot.trim_end().ends_with('}'));
    }

    #[test]
    fn render_dot_emits_one_node_per_unique_id_and_edges_for_parents() {
        let p_empty: Vec<String> = vec![];
        let p_tool1: Vec<String> = vec!["tool:1".into()];
        let rows = vec![
            ProvenanceRow {
                node_id: "tool:1",
                parents: &p_empty,
                op: "tool_call:get_order",
                label: None,
            },
            ProvenanceRow {
                node_id: "llm:2",
                parents: &p_tool1,
                op: "llm:classify",
                label: None,
            },
        ];
        let dot = render_dot(&rows);
        assert!(dot.contains("\"tool:1\" [label="), "got: {dot}");
        assert!(dot.contains("\"llm:2\" [label="), "got: {dot}");
        assert!(dot.contains("\"tool:1\" -> \"llm:2\";"), "got: {dot}");
        // Node count: `tool:1` declared once despite being both a
        // row and a parent.
        let tool_declarations = dot.matches("\"tool:1\" [label=").count();
        assert_eq!(tool_declarations, 1);
    }

    #[test]
    fn render_dot_prefers_user_label_over_op_when_present() {
        let parents: Vec<String> = vec![];
        let rows = vec![ProvenanceRow {
            node_id: "llm:3",
            parents: &parents,
            op: "llm:classify",
            label: Some("ticket classifier"),
        }];
        let dot = render_dot(&rows);
        assert!(
            dot.contains("label=\"ticket classifier\""),
            "user label should win over op, got: {dot}"
        );
        assert!(
            !dot.contains("llm:classify"),
            "op should not appear when user label is present, got: {dot}"
        );
    }

    #[test]
    fn render_dot_declares_parent_nodes_not_otherwise_listed() {
        // "tool:99" is a parent referenced by "llm:4" but never
        // appears as its own ProvenanceEdge row. The renderer must
        // still declare it as a bare node so DOT doesn't fail.
        let parents: Vec<String> = vec!["tool:99".into()];
        let rows = vec![ProvenanceRow {
            node_id: "llm:4",
            parents: &parents,
            op: "llm:sum",
            label: None,
        }];
        let dot = render_dot(&rows);
        assert!(dot.contains("\"tool:99\" [label="), "got: {dot}");
    }

    #[test]
    fn quote_dot_escapes_embedded_quotes_and_backslashes() {
        assert_eq!(quote_dot("ab"), r#""ab""#);
        assert_eq!(quote_dot("a\"b"), r#""a\"b""#);
        assert_eq!(quote_dot("a\\b"), r#""a\\b""#);
    }
}
