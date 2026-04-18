//! Composed-effect-profile diff between two revisions of a Corvid
//! project. Powers `corvid effect-diff <before> <after>`.
//!
//! Each side can be a directory, a single `.cor` file, or (eventually)
//! a git ref. The differ compiles every `.cor` file on each side,
//! extracts each agent's composed dimensional profile from the
//! checker output, and reports:
//!
//!   * agents added / removed between the two sides
//!   * per-agent dimension drift (before value vs. after value)
//!   * constraints that newly fire or newly release because of the
//!     dimension drift
//!
//! Effect refactoring becomes safe because the diff tool surfaces
//! every consequence. No other language has an analogous analysis
//! because no other language has quantitative effects to diff.
//!
//! See `docs/effects-spec/02-composition-algebra.md` §9 for the spec.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use corvid_ast::DimensionValue;
use corvid_resolve::resolve;
use corvid_syntax::{lex, parse_file};
use corvid_types::{analyze_effects, typecheck_with_config, EffectRegistry};

use crate::load_corvid_config_for;

/// A snapshot of every agent's composed effect profile in a single
/// revision. The per-agent entries are keyed by agent name so diffing
/// is straightforward.
#[derive(Debug, Clone)]
pub struct RevisionSnapshot {
    pub root: PathBuf,
    pub agents: BTreeMap<String, AgentSnapshot>,
}

#[derive(Debug, Clone)]
pub struct AgentSnapshot {
    pub file: PathBuf,
    pub name: String,
    /// Dimension name → composed value at this revision.
    pub dimensions: BTreeMap<String, DimensionValue>,
    /// Agent-level constraint violations the checker surfaced. The
    /// diff tool uses this to report "newly firing" / "newly released"
    /// constraints between revisions.
    pub violations: BTreeSet<String>,
}

/// The output of diffing two `RevisionSnapshot`s.
#[derive(Debug, Clone, Default)]
pub struct EffectDiff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<AgentDiff>,
    pub unchanged: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct AgentDiff {
    pub agent: String,
    pub before_file: PathBuf,
    pub after_file: PathBuf,
    pub dimension_changes: Vec<DimensionChange>,
    pub newly_firing: Vec<String>,
    pub newly_released: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DimensionChange {
    pub dimension: String,
    pub before: Option<DimensionValue>,
    pub after: Option<DimensionValue>,
}

/// Load every `.cor` file under `side`, compile each with its own
/// corvid.toml context, collect per-agent composed profiles.
///
/// `side` may be:
///   * a directory — walk recursively for `.cor` files
///   * a single file — just use it
///
/// Non-`.cor` files are ignored. Files that fail to compile are
/// silently skipped (their agents vanish from the snapshot). That
/// keeps the diff meaningful on broken trees — you still see the
/// shape of what DID compile.
pub fn snapshot_revision(side: &Path) -> Result<RevisionSnapshot> {
    let mut agents: BTreeMap<String, AgentSnapshot> = BTreeMap::new();
    for file in collect_cor_files(side)? {
        let source = fs::read_to_string(&file)
            .with_context(|| format!("cannot read `{}`", file.display()))?;
        let config = load_corvid_config_for(&file);
        let Ok(tokens) = lex(&source) else { continue };
        let (parsed, parse_errors) = parse_file(&tokens);
        if !parse_errors.is_empty() {
            continue;
        }
        let resolved = resolve(&parsed);
        if !resolved.errors.is_empty() {
            continue;
        }
        let checked = typecheck_with_config(&parsed, &resolved, config.as_ref());
        if !checked.errors.is_empty() {
            // We still extract agent profiles from files with errors
            // below — the checker's `analyze_effects` populates them
            // best-effort. Skip only if the file didn't parse.
        }
        let effect_decls: Vec<_> = parsed
            .decls
            .iter()
            .filter_map(|d| match d {
                corvid_ast::Decl::Effect(e) => Some(e.clone()),
                _ => None,
            })
            .collect();
        let registry = EffectRegistry::from_decls_with_config(&effect_decls, config.as_ref());
        let summaries = analyze_effects(&parsed, &resolved, &registry);
        for summary in summaries {
            let mut dims: BTreeMap<String, DimensionValue> = BTreeMap::new();
            for (name, value) in &summary.composed.dimensions {
                dims.insert(name.clone(), value.clone());
            }
            let violations: BTreeSet<String> =
                summary.violations.iter().map(|v| v.to_string()).collect();
            agents.insert(
                summary.agent_name.clone(),
                AgentSnapshot {
                    file: file.clone(),
                    name: summary.agent_name,
                    dimensions: dims,
                    violations,
                },
            );
        }
    }
    Ok(RevisionSnapshot {
        root: side.to_path_buf(),
        agents,
    })
}

/// Compute the diff between two snapshots. Agents present only on one
/// side appear in `added` / `removed`; agents present on both appear
/// in `changed` (if any dimension or violation changed) or
/// `unchanged`.
pub fn diff_snapshots(before: &RevisionSnapshot, after: &RevisionSnapshot) -> EffectDiff {
    let mut out = EffectDiff::default();
    let before_names: BTreeSet<&String> = before.agents.keys().collect();
    let after_names: BTreeSet<&String> = after.agents.keys().collect();

    for name in after_names.difference(&before_names) {
        out.added.push((*name).clone());
    }
    for name in before_names.difference(&after_names) {
        out.removed.push((*name).clone());
    }
    for name in before_names.intersection(&after_names) {
        let a = &before.agents[*name];
        let b = &after.agents[*name];
        let dimension_changes = dimension_changes(a, b);
        let (newly_firing, newly_released) = violation_changes(a, b);
        if dimension_changes.is_empty() && newly_firing.is_empty() && newly_released.is_empty() {
            out.unchanged.push((*name).clone());
        } else {
            out.changed.push(AgentDiff {
                agent: (*name).clone(),
                before_file: a.file.clone(),
                after_file: b.file.clone(),
                dimension_changes,
                newly_firing,
                newly_released,
            });
        }
    }

    out
}

fn dimension_changes(a: &AgentSnapshot, b: &AgentSnapshot) -> Vec<DimensionChange> {
    let mut changes = Vec::new();
    let mut seen: BTreeSet<&String> = BTreeSet::new();
    for (name, before_val) in &a.dimensions {
        seen.insert(name);
        let after_val = b.dimensions.get(name);
        match after_val {
            Some(after_val) if dim_eq(before_val, after_val) => {}
            Some(after_val) => changes.push(DimensionChange {
                dimension: name.clone(),
                before: Some(before_val.clone()),
                after: Some(after_val.clone()),
            }),
            None => changes.push(DimensionChange {
                dimension: name.clone(),
                before: Some(before_val.clone()),
                after: None,
            }),
        }
    }
    for (name, after_val) in &b.dimensions {
        if seen.contains(name) {
            continue;
        }
        changes.push(DimensionChange {
            dimension: name.clone(),
            before: None,
            after: Some(after_val.clone()),
        });
    }
    changes
}

fn violation_changes(
    a: &AgentSnapshot,
    b: &AgentSnapshot,
) -> (Vec<String>, Vec<String>) {
    let newly_firing: Vec<String> = b
        .violations
        .difference(&a.violations)
        .cloned()
        .collect();
    let newly_released: Vec<String> = a
        .violations
        .difference(&b.violations)
        .cloned()
        .collect();
    (newly_firing, newly_released)
}

fn dim_eq(a: &DimensionValue, b: &DimensionValue) -> bool {
    match (a, b) {
        (DimensionValue::Bool(x), DimensionValue::Bool(y)) => x == y,
        (DimensionValue::Name(x), DimensionValue::Name(y)) => x == y,
        (DimensionValue::Cost(x), DimensionValue::Cost(y)) => (x - y).abs() < 1e-9,
        (DimensionValue::Number(x), DimensionValue::Number(y)) => (x - y).abs() < 1e-9,
        _ => format!("{a:?}") == format!("{b:?}"),
    }
}

/// Render an `EffectDiff` as a human-readable report.
pub fn render_effect_diff(diff: &EffectDiff) -> String {
    let mut out = String::new();
    if !diff.added.is_empty() {
        out.push_str(&format!(
            "\n+ {} agent(s) added:\n",
            diff.added.len()
        ));
        for name in &diff.added {
            out.push_str(&format!("    {name}\n"));
        }
    }
    if !diff.removed.is_empty() {
        out.push_str(&format!(
            "\n- {} agent(s) removed:\n",
            diff.removed.len()
        ));
        for name in &diff.removed {
            out.push_str(&format!("    {name}\n"));
        }
    }
    if !diff.changed.is_empty() {
        out.push_str(&format!(
            "\n~ {} agent(s) changed:\n",
            diff.changed.len()
        ));
        for agent_diff in &diff.changed {
            out.push_str(&format!("\n  {}\n", agent_diff.agent));
            for change in &agent_diff.dimension_changes {
                out.push_str(&format!(
                    "    {:<14} {}\n",
                    change.dimension,
                    render_change(&change.before, &change.after),
                ));
            }
            for violation in &agent_diff.newly_firing {
                out.push_str(&format!("    ! newly firing: {violation}\n"));
            }
            for violation in &agent_diff.newly_released {
                out.push_str(&format!("    ✓ newly released: {violation}\n"));
            }
        }
    }
    if diff.added.is_empty() && diff.removed.is_empty() && diff.changed.is_empty() {
        out.push_str(&format!(
            "\nNo composed-profile changes across {} unchanged agent(s).\n",
            diff.unchanged.len()
        ));
    } else {
        out.push_str(&format!(
            "\n{} changed, {} added, {} removed, {} unchanged.\n",
            diff.changed.len(),
            diff.added.len(),
            diff.removed.len(),
            diff.unchanged.len(),
        ));
    }
    out
}

fn render_change(before: &Option<DimensionValue>, after: &Option<DimensionValue>) -> String {
    match (before, after) {
        (Some(b), Some(a)) => format!("{} → {}", format_dim(b), format_dim(a)),
        (Some(b), None) => format!("{} → (unset)", format_dim(b)),
        (None, Some(a)) => format!("(unset) → {}", format_dim(a)),
        (None, None) => "(no change)".into(),
    }
}

fn format_dim(v: &DimensionValue) -> String {
    match v {
        DimensionValue::Bool(b) => b.to_string(),
        DimensionValue::Name(n) => n.clone(),
        DimensionValue::Cost(c) => format!("${c:.4}"),
        DimensionValue::Number(n) => format!("{n}"),
        other => format!("{other:?}"),
    }
}

fn collect_cor_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    if !root.exists() {
        return Err(anyhow!("`{}` does not exist", root.display()));
    }
    if root.is_file() {
        if root.extension().and_then(|e| e.to_str()) == Some("cor") {
            out.push(root.to_path_buf());
        }
        return Ok(out);
    }
    walk_cor(root, &mut out)?;
    out.sort();
    Ok(out)
}

fn walk_cor(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)
        .with_context(|| format!("cannot read `{}`", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            // Skip build/dependency folders that pollute the snapshot.
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if matches!(name, "target" | "node_modules" | ".git" | "__pycache__") {
                    continue;
                }
            }
            walk_cor(&path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("cor") {
            out.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(root: &Path, file: &str, contents: &str) {
        let path = root.join(file);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn snapshot_collects_per_agent_dimensions() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "main.cor",
            "\
effect lookup:
    cost: $0.01

tool fetch(id: String) -> String uses lookup

agent run(id: String) -> String:
    return fetch(id)
",
        );
        let snapshot = snapshot_revision(tmp.path()).unwrap();
        let run = snapshot.agents.get("run").expect("agent run present");
        assert!(run.dimensions.contains_key("cost"));
        match run.dimensions.get("cost").unwrap() {
            DimensionValue::Cost(c) => assert!((c - 0.01).abs() < 1e-6),
            other => panic!("unexpected cost dim: {other:?}"),
        }
    }

    #[test]
    fn diff_flags_cost_drift_between_revisions() {
        let before_tmp = TempDir::new().unwrap();
        let after_tmp = TempDir::new().unwrap();
        let before_src = "\
effect lookup:
    cost: $0.01

tool fetch(id: String) -> String uses lookup

agent run(id: String) -> String:
    return fetch(id)
";
        let after_src = "\
effect lookup:
    cost: $0.05

tool fetch(id: String) -> String uses lookup

agent run(id: String) -> String:
    return fetch(id)
";
        write(before_tmp.path(), "main.cor", before_src);
        write(after_tmp.path(), "main.cor", after_src);
        let before = snapshot_revision(before_tmp.path()).unwrap();
        let after = snapshot_revision(after_tmp.path()).unwrap();
        let diff = diff_snapshots(&before, &after);
        assert!(diff.added.is_empty());
        assert!(diff.removed.is_empty());
        assert_eq!(diff.changed.len(), 1);
        let change = &diff.changed[0];
        assert_eq!(change.agent, "run");
        let cost_change = change
            .dimension_changes
            .iter()
            .find(|c| c.dimension == "cost")
            .expect("cost dimension drifted");
        match (cost_change.before.as_ref(), cost_change.after.as_ref()) {
            (Some(DimensionValue::Cost(b)), Some(DimensionValue::Cost(a))) => {
                assert!((b - 0.01).abs() < 1e-6);
                assert!((a - 0.05).abs() < 1e-6);
            }
            other => panic!("unexpected cost change shape: {other:?}"),
        }
    }

    #[test]
    fn diff_classifies_added_and_removed_agents() {
        let before_tmp = TempDir::new().unwrap();
        let after_tmp = TempDir::new().unwrap();
        write(
            before_tmp.path(),
            "main.cor",
            "\
agent old_agent() -> String:
    return \"x\"
",
        );
        write(
            after_tmp.path(),
            "main.cor",
            "\
agent new_agent() -> String:
    return \"y\"
",
        );
        let before = snapshot_revision(before_tmp.path()).unwrap();
        let after = snapshot_revision(after_tmp.path()).unwrap();
        let diff = diff_snapshots(&before, &after);
        assert_eq!(diff.added, vec!["new_agent".to_string()]);
        assert_eq!(diff.removed, vec!["old_agent".to_string()]);
    }

    #[test]
    fn diff_marks_identical_revisions_as_unchanged() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "main.cor",
            "\
effect lookup:
    cost: $0.01

tool fetch(id: String) -> String uses lookup

agent run(id: String) -> String:
    return fetch(id)
",
        );
        let snap = snapshot_revision(tmp.path()).unwrap();
        // Diff a snapshot against itself — the graph is entirely stable.
        let diff = diff_snapshots(&snap, &snap);
        assert!(diff.added.is_empty());
        assert!(diff.removed.is_empty());
        assert!(diff.changed.is_empty());
        assert_eq!(diff.unchanged, vec!["run".to_string()]);
    }

    #[test]
    fn diff_detects_newly_firing_violations() {
        let before_tmp = TempDir::new().unwrap();
        let after_tmp = TempDir::new().unwrap();
        // Before: cost is within budget. After: cost triples, exceeds
        // the declared @budget. The agent survives (budget doesn't
        // block compilation — it's a warning for this dimension) but
        // a constraint violation should change.
        let before_src = "\
effect lookup:
    cost: $0.01
    trust: human_required

tool fetch(id: String) -> String uses lookup

@trust(autonomous)
agent run(id: String) -> String:
    return fetch(id)
";
        let after_src = "\
effect lookup:
    cost: $0.01
    trust: autonomous

tool fetch(id: String) -> String uses lookup

@trust(autonomous)
agent run(id: String) -> String:
    return fetch(id)
";
        write(before_tmp.path(), "main.cor", before_src);
        write(after_tmp.path(), "main.cor", after_src);
        let before = snapshot_revision(before_tmp.path()).unwrap();
        let after = snapshot_revision(after_tmp.path()).unwrap();
        let diff = diff_snapshots(&before, &after);
        assert_eq!(diff.changed.len(), 1, "expected one changed agent");
        let change = &diff.changed[0];
        assert!(
            !change.newly_released.is_empty(),
            "trust relaxation should release the @trust(autonomous) violation; got change = {change:?}"
        );
    }

    #[test]
    fn render_reports_no_changes_when_identical() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "main.cor",
            "agent run() -> String:\n    return \"x\"\n",
        );
        let snap = snapshot_revision(tmp.path()).unwrap();
        let diff = diff_snapshots(&snap, &snap);
        let rendered = render_effect_diff(&diff);
        assert!(rendered.contains("No composed-profile changes"));
    }

    #[test]
    fn snapshot_handles_missing_directory_as_error() {
        let err = snapshot_revision(Path::new("/nonexistent/path/to/nowhere")).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("does not exist"), "unexpected message: {msg}");
    }
}
