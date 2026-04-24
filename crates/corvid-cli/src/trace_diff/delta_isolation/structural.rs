//! Per-class isolators for structural delta classes — deltas
//! that correspond to whole declaration blocks or statement
//! insertions/removals, not single-token annotation changes.
//!
//! **Honest scope of the first sub-commit (`3c-structural-
//! lifecycle`):**
//!
//! - `agent.added:X` — splice the commit's full agent
//!   declaration block (including leading attributes, visibility
//!   prefix, body) into parent source at end-of-source.
//! - `agent.removed:X` — splice parent's full agent declaration
//!   block out of parent source.
//!
//! Approval-label and provenance-grounded classes land in
//! subsequent sub-commits of `3c-structural`, each gated on its
//! own `probe_round_trip` confirming its ABI delta fires.
//!
//! Provenance-dep deltas (`dep_added` / `dep_removed`) are
//! *derived* from body analysis in
//! `corvid-abi/src/provenance_emit.rs` — they aren't a
//! source-level annotation the isolator can flip; they change
//! when the agent's body changes in ways that affect which
//! parameters the grounding depends on. Those belong with
//! `dangerous_gained` in the next sub-slice
//! (`3c-annotation-or-derived`), not here. Documented so future-
//! me doesn't re-discover the same invariant.

use corvid_ast::{AgentDecl, Span};

use super::ast_helpers::find_agent;
use super::{IsolationError, IsolationInput, SpliceOp};

/// Isolate a `agent.added:X` or `agent.removed:X` delta by
/// splicing the full declaration block into / out of parent
/// source.
pub(super) fn isolate_lifecycle(
    delta_key: &str,
    parent: &IsolationInput,
    commit: &IsolationInput,
) -> Result<Vec<SpliceOp>, IsolationError> {
    let (agent_name, is_added) = if let Some(name) = delta_key.strip_prefix("agent.added:") {
        (name, true)
    } else if let Some(name) = delta_key.strip_prefix("agent.removed:") {
        (name, false)
    } else {
        return Err(IsolationError::UnsupportedDeltaClass(delta_key.to_string()));
    };

    if is_added {
        // Commit has X; parent doesn't. Copy commit's full
        // declaration block (with its leading attributes +
        // visibility prefix + body) to parent, appended at
        // end-of-source with a blank-line separator. End-of-
        // source is the CTO-locked deterministic position:
        // source order within the ABI's agents vec doesn't
        // affect the `agent.added:X` delta key (narrative.rs
        // keys by name), so end-of-source maximizes byte-
        // stability of parent's existing content.
        let commit_agent = find_agent(&commit.file, agent_name).ok_or_else(|| {
            IsolationError::AgentNotFound {
                name: agent_name.to_string(),
                side: "commit",
            }
        })?;
        let block_text = extract_declaration_block(&commit.source, commit_agent);

        // Insert at the end of parent, preceded by a blank-line
        // separator unless parent already ends with one.
        let parent_len = parent.source.len();
        let prefix = trailing_separator(&parent.source);
        let replacement = format!("{prefix}{block_text}");
        Ok(vec![SpliceOp {
            range: parent_len..parent_len,
            replacement,
        }])
    } else {
        // Parent has X; commit doesn't. Splice parent's full
        // declaration block out (plus trailing newline / blank
        // line so the remaining source doesn't keep a gap).
        let parent_agent = find_agent(&parent.file, agent_name).ok_or_else(|| {
            IsolationError::AgentNotFound {
                name: agent_name.to_string(),
                side: "parent",
            }
        })?;
        let block_range = declaration_block_range(&parent.source, parent_agent);
        Ok(vec![SpliceOp {
            range: block_range,
            replacement: String::new(),
        }])
    }
}

/// Extract the full source text of an agent's declaration block,
/// including any leading attributes + visibility prefix (`pub
/// extern "c"`) on the declaration line. The AST's
/// `agent.span.start` points at the `agent` keyword, so we need
/// to scan backward to the start of the line containing
/// `span.start` — that's where the attributes / visibility-
/// prefix chain begins. End of block = `agent.span.end`.
fn extract_declaration_block(source: &str, agent: &AgentDecl) -> String {
    let block_start = find_declaration_block_start(source, agent);
    let block_end = agent.span.end;
    source[block_start..block_end].to_string()
}

/// Compute the full byte-range of an agent's declaration block
/// in `source`, including leading attributes / visibility prefix
/// AND trailing whitespace up to (and including) the next
/// newline. Used for clean removal — leaves no orphaned blank
/// lines.
fn declaration_block_range(source: &str, agent: &AgentDecl) -> std::ops::Range<usize> {
    let block_start = find_declaration_block_start(source, agent);
    let mut block_end = agent.span.end;
    let bytes = source.as_bytes();
    // Extend through trailing whitespace + one newline.
    while block_end < bytes.len() {
        match bytes[block_end] {
            b'\n' => {
                block_end += 1;
                break;
            }
            b' ' | b'\t' | b'\r' => block_end += 1,
            _ => break,
        }
    }
    block_start..block_end
}

/// Find the byte offset at which an agent declaration *block*
/// begins — that is, the line containing the earliest of the
/// agent's attributes / visibility prefix / `agent` keyword.
fn find_declaration_block_start(source: &str, agent: &AgentDecl) -> usize {
    // Earliest span among the agent's attributes, constraints,
    // and the agent's own span. Attributes like `@replayable`
    // come first in source order; constraints like
    // `@trust(...)` interleave; visibility prefix (`pub extern
    // "c"`) isn't separately spanned — we fall back to scanning
    // backward from the earliest known span to the line start.
    let mut earliest = agent.span.start;
    for attr in &agent.attributes {
        earliest = earliest.min(attr.span().start);
    }
    for constraint in &agent.constraints {
        earliest = earliest.min(constraint.span.start);
    }
    // Back up through any leading visibility prefix on the same
    // line (`pub extern "c"`, or `public`, etc.) and any blank
    // leading whitespace — scan to the line start.
    find_line_start(source, earliest)
}

fn find_line_start(source: &str, offset: usize) -> usize {
    let bytes = source.as_bytes();
    let mut i = offset;
    while i > 0 {
        if bytes[i - 1] == b'\n' {
            return i;
        }
        i -= 1;
    }
    0
}

/// Compute the separator bytes to prepend when appending a new
/// declaration at the end of `source`. Returns `"\n\n"` if
/// source doesn't end with a newline, `"\n"` if it ends with a
/// single newline (so we produce a blank line before the new
/// block), or `""` if source already ends with a blank line.
fn trailing_separator(source: &str) -> &'static str {
    let bytes = source.as_bytes();
    let n = bytes.len();
    if n == 0 {
        return "";
    }
    // Ends with "\n\n"? Already blank-line-terminated.
    if n >= 2 && bytes[n - 1] == b'\n' && bytes[n - 2] == b'\n' {
        return "";
    }
    // Ends with "\n" (single)? Add one more newline for blank line.
    if bytes[n - 1] == b'\n' {
        return "\n";
    }
    // Doesn't end in newline at all. Add two.
    "\n\n"
}

// Silence unused warning for Span during this sub-commit until
// approval-label / grounded classes wire it in.
#[allow(dead_code)]
type _UnusedSpan = Span;

// ---------------------------------------------------------------
// Three-test triad per delta class
// ---------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::{apply_splices, compute_splices_for_delta, prepare_isolation_input};
    use super::super::ast_helpers::probe_round_trip;
    use crate::trace_diff::narrative::compute_diff_summary;
    use corvid_driver::compile_to_abi_with_config;

    const GENERATED_AT: &str = "1970-01-01T00:00:00Z";

    fn compile(source: &str) -> corvid_abi::CorvidAbi {
        compile_to_abi_with_config(source, "test.cor", GENERATED_AT, None)
            .unwrap_or_else(|diags| panic!("compile failed: {} diagnostic(s)", diags.len()))
    }

    // -----------------------------------------------------------
    // agent.added:X
    // -----------------------------------------------------------

    const ADDED_PARENT: &str = r#"
pub extern "c" agent greet() -> Int:
    return 1
"#;
    const ADDED_COMMIT: &str = r#"
pub extern "c" agent greet() -> Int:
    return 1

pub extern "c" agent summarize() -> Int:
    return 2
"#;

    #[test]
    fn agent_added_probe_confirms_delta_fires() {
        // Pre-implementation probe: the delta key the class
        // claims to isolate must actually appear in the
        // parent->commit diff on the curated fixture.
        assert!(probe_round_trip(
            ADDED_PARENT,
            ADDED_COMMIT,
            "agent.added:summarize",
        ));
    }

    #[test]
    fn agent_added_apply_succeeds() {
        let parent = prepare_isolation_input(ADDED_PARENT).unwrap();
        let commit = prepare_isolation_input(ADDED_COMMIT).unwrap();
        let splices =
            compute_splices_for_delta("agent.added:summarize", &parent, &commit).unwrap();
        let out = apply_splices(ADDED_PARENT, splices).unwrap();
        assert!(out.contains("agent summarize"));
        assert!(out.contains("agent greet"));
    }

    #[test]
    fn agent_added_byte_stability_for_unchanged_regions() {
        let parent = prepare_isolation_input(ADDED_PARENT).unwrap();
        let commit = prepare_isolation_input(ADDED_COMMIT).unwrap();
        let splices =
            compute_splices_for_delta("agent.added:summarize", &parent, &commit).unwrap();
        let out = apply_splices(ADDED_PARENT, splices).unwrap();
        // Parent's existing content must come through byte-for-
        // byte. The new agent gets appended after.
        assert!(
            out.starts_with(ADDED_PARENT),
            "parent content must be a prefix of synthesized output; got:\n{out}"
        );
    }

    #[test]
    fn agent_added_roundtrip_matches_expected_abi_delta() {
        let parent_abi = compile(ADDED_PARENT);
        let parent_input = prepare_isolation_input(ADDED_PARENT).unwrap();
        let commit_input = prepare_isolation_input(ADDED_COMMIT).unwrap();
        let splices = compute_splices_for_delta(
            "agent.added:summarize",
            &parent_input,
            &commit_input,
        )
        .unwrap();
        let synthesized = apply_splices(ADDED_PARENT, splices).unwrap();
        let synth_abi = compile(&synthesized);
        let synth_diff = compute_diff_summary(&parent_abi, &synth_abi);
        assert!(
            synth_diff
                .records
                .iter()
                .any(|r| r.key == "agent.added:summarize"),
            "isolator must produce the target delta; got: {:?}",
            synth_diff.records.iter().map(|r| &r.key).collect::<Vec<_>>()
        );
        // The new agent's default contract (no attributes,
        // no approvals, no grounded return) may surface initial-
        // contract deltas alongside the lifecycle delta — that's
        // emit_initial_contract behavior. We assert the target
        // key is present, not that it's the only one.
    }

    // -----------------------------------------------------------
    // agent.removed:X
    // -----------------------------------------------------------

    #[test]
    fn agent_removed_probe_confirms_delta_fires() {
        // Swap roles: parent has both agents; commit has only
        // greet. The diff parent->commit should emit
        // agent.removed:summarize.
        let parent_src = ADDED_COMMIT;
        let commit_src = ADDED_PARENT;
        assert!(probe_round_trip(
            parent_src,
            commit_src,
            "agent.removed:summarize",
        ));
    }

    #[test]
    fn agent_removed_apply_succeeds() {
        let parent_src = ADDED_COMMIT;
        let commit_src = ADDED_PARENT;
        let parent = prepare_isolation_input(parent_src).unwrap();
        let commit = prepare_isolation_input(commit_src).unwrap();
        let splices =
            compute_splices_for_delta("agent.removed:summarize", &parent, &commit).unwrap();
        let out = apply_splices(parent_src, splices).unwrap();
        assert!(
            !out.contains("agent summarize"),
            "synthesized source must not contain the removed agent; got:\n{out}"
        );
        assert!(
            out.contains("agent greet"),
            "other agent must survive; got:\n{out}"
        );
    }

    #[test]
    fn agent_removed_byte_stability_for_unchanged_regions() {
        // The surviving `greet` agent's source lines must come
        // through byte-identical. Declaring the property
        // explicitly: `pub extern "c" agent greet() -> Int:
        // return 1` is present in both parent and synthesized.
        let parent_src = ADDED_COMMIT;
        let commit_src = ADDED_PARENT;
        let parent = prepare_isolation_input(parent_src).unwrap();
        let commit = prepare_isolation_input(commit_src).unwrap();
        let splices =
            compute_splices_for_delta("agent.removed:summarize", &parent, &commit).unwrap();
        let out = apply_splices(parent_src, splices).unwrap();
        assert!(
            out.contains("pub extern \"c\" agent greet() -> Int:\n    return 1\n"),
            "surviving agent body must come through byte-identical; got:\n{out}"
        );
    }

    #[test]
    fn agent_removed_roundtrip_matches_expected_abi_delta() {
        let parent_src = ADDED_COMMIT;
        let parent_abi = compile(parent_src);
        let parent_input = prepare_isolation_input(parent_src).unwrap();
        let commit_input = prepare_isolation_input(ADDED_PARENT).unwrap();
        let splices = compute_splices_for_delta(
            "agent.removed:summarize",
            &parent_input,
            &commit_input,
        )
        .unwrap();
        let synthesized = apply_splices(parent_src, splices).unwrap();
        let synth_abi = compile(&synthesized);
        let synth_diff = compute_diff_summary(&parent_abi, &synth_abi);
        assert!(
            synth_diff
                .records
                .iter()
                .any(|r| r.key == "agent.removed:summarize"),
            "isolator must produce the target delta; got: {:?}",
            synth_diff.records.iter().map(|r| &r.key).collect::<Vec<_>>()
        );
    }

    // -----------------------------------------------------------
    // Helper tests
    // -----------------------------------------------------------

    #[test]
    fn trailing_separator_handles_various_endings() {
        assert_eq!(trailing_separator(""), "");
        assert_eq!(trailing_separator("hello"), "\n\n");
        assert_eq!(trailing_separator("hello\n"), "\n");
        assert_eq!(trailing_separator("hello\n\n"), "");
    }

    #[test]
    fn find_line_start_of_first_line_is_zero() {
        assert_eq!(find_line_start("pub extern...", 4), 0);
    }

    #[test]
    fn find_line_start_scans_back_to_previous_newline() {
        let source = "line1\npub extern \"c\" agent";
        // offset 12 is somewhere in "pub extern..."; line start
        // is the byte after the '\n' at offset 5.
        assert_eq!(find_line_start(source, 12), 6);
    }
}
