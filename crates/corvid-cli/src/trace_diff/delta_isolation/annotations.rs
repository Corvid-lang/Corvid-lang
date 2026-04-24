//! Per-class isolators for annotation-style deltas.
//!
//! These are the simplest delta classes: each corresponds to a
//! single AST node with a known byte range in source, so the
//! isolator's job is to locate the node in parent (or commit)
//! source, expand its span to a clean splice range, and copy the
//! corresponding text from the other side.
//!
//! **Honest scope of this sub-commit (3c-annotations step 1):**
//!
//! - `agent.replayable_gained:X` / `agent.replayable_lost:X` —
//!   `@replayable` attribute on an agent declaration. Full three-
//!   test triad per class (apply succeeds; byte-stability for
//!   unchanged regions; round-trip through the ABI matches the
//!   expected delta).
//!
//! **Deferred to a follow-up sub-commit** (`3c-annotations-trust-
//! tier` or similar): `agent.trust_tier_changed`. The
//! `@trust(tier)` constraint only affects the emitted ABI's
//! `trust_tier` field when the agent's body has
//! trust-composing effects to apply the constraint to — an empty-
//! body `return 1` agent's ABI is unchanged by `@trust(A)` vs
//! `@trust(B)`. Shipping the isolator without a confirmable
//! round-trip test would be the exact kind of shortcut the
//! per-class-triad discipline is designed to prevent. A follow-up
//! commit will build the test against a body with real effects.
//!
//! Other delta classes (`@dangerous` derived, approval label
//! add/remove, provenance, lifecycle) live in their own modules
//! in later sub-commits.

use corvid_ast::{AgentAttribute, AgentDecl};

use super::ast_helpers::find_agent;
use super::{IsolationError, IsolationInput, SpliceOp};

/// Isolate a `replayable_gained` or `replayable_lost` delta by
/// computing splice ops that add/remove the `@replayable`
/// attribute on the named agent.
pub(super) fn isolate_replayable(
    delta_key: &str,
    parent: &IsolationInput,
    commit: &IsolationInput,
) -> Result<Vec<SpliceOp>, IsolationError> {
    let (agent_name, is_gained) = if let Some(name) =
        delta_key.strip_prefix("agent.replayable_gained:")
    {
        (name, true)
    } else if let Some(name) = delta_key.strip_prefix("agent.replayable_lost:") {
        (name, false)
    } else {
        return Err(IsolationError::UnsupportedDeltaClass(delta_key.to_string()));
    };

    if is_gained {
        // Commit has `@replayable`; parent doesn't. Copy commit's
        // attribute text + its trailing whitespace into parent at
        // the agent declaration's start position.
        let commit_agent = find_agent(&commit.file, agent_name).ok_or_else(|| {
            IsolationError::AgentNotFound {
                name: agent_name.to_string(),
                side: "commit",
            }
        })?;
        let attr_span = find_replayable_attribute(commit_agent).ok_or_else(|| {
            IsolationError::ExpectedStateNotFound {
                detail: format!(
                    "commit source for agent `{agent_name}` should carry `@replayable`"
                ),
            }
        })?;
        let expanded = expand_to_trailing_newline(&commit.source, attr_span.start, attr_span.end);
        let attribute_text = commit.source[expanded].to_string();

        let parent_agent = find_agent(&parent.file, agent_name).ok_or_else(|| {
            IsolationError::AgentNotFound {
                name: agent_name.to_string(),
                side: "parent",
            }
        })?;
        // `agent.span.start` points at the `agent` keyword, which
        // is AFTER any `pub extern "c"` prefix on the declaration
        // line. The attribute must go at the START of the line,
        // before the visibility prefix — otherwise the splice
        // produces ill-formed source like `pub extern "c"
        // @replayable\nagent greet()...`.
        let insert_at = find_line_start(&parent.source, parent_agent.span.start);
        Ok(vec![SpliceOp {
            range: insert_at..insert_at,
            replacement: attribute_text,
        }])
    } else {
        // Parent has `@replayable`; commit doesn't. Delete
        // parent's attribute span + trailing whitespace.
        let parent_agent = find_agent(&parent.file, agent_name).ok_or_else(|| {
            IsolationError::AgentNotFound {
                name: agent_name.to_string(),
                side: "parent",
            }
        })?;
        let attr_span = find_replayable_attribute(parent_agent).ok_or_else(|| {
            IsolationError::ExpectedStateNotFound {
                detail: format!(
                    "parent source for agent `{agent_name}` should carry `@replayable`"
                ),
            }
        })?;
        let expanded = expand_to_trailing_newline(&parent.source, attr_span.start, attr_span.end);
        Ok(vec![SpliceOp {
            range: expanded,
            replacement: String::new(),
        }])
    }
}

// ---------------------------------------------------------------
// Class-specific AST helpers (shared helpers live in `ast_helpers`)
// ---------------------------------------------------------------

fn find_replayable_attribute(agent: &AgentDecl) -> Option<corvid_ast::Span> {
    agent.attributes.iter().find_map(|attr| match attr {
        AgentAttribute::Replayable { span } => Some(*span),
        _ => None,
    })
}

/// Find the byte offset of the start of the line containing
/// `offset`. Returns 0 when `offset` is on the first line. Used
/// to anchor attribute insertions at the beginning of the
/// declaration line, before any `pub extern "c"` visibility
/// prefix that precedes the `agent` keyword.
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

/// Expand a span forward to include trailing whitespace up to and
/// including the first newline. Used when removing an attribute
/// so the source doesn't keep a blank line behind.
fn expand_to_trailing_newline(
    source: &str,
    start: usize,
    initial_end: usize,
) -> std::ops::Range<usize> {
    let bytes = source.as_bytes();
    let mut end = initial_end;
    while end < bytes.len() {
        match bytes[end] {
            b'\n' => {
                end += 1;
                break;
            }
            b' ' | b'\t' | b'\r' => end += 1,
            _ => break,
        }
    }
    start..end
}

// ---------------------------------------------------------------
// Three-test triad per delta class
// (CTO-locked discipline: apply succeeds; byte-stability for
// unchanged regions; round-trip through the ABI matches the
// expected delta.)
// ---------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::{apply_splices, compute_splices_for_delta, prepare_isolation_input};
    use crate::trace_diff::narrative::compute_diff_summary;
    use corvid_driver::compile_to_abi_with_config;

    const GENERATED_AT: &str = "1970-01-01T00:00:00Z";

    fn compile(source: &str) -> corvid_abi::CorvidAbi {
        compile_to_abi_with_config(source, "test.cor", GENERATED_AT, None)
            .unwrap_or_else(|diags| panic!("compile failed: {} diagnostic(s)", diags.len()))
    }

    // -----------------------------------------------------------
    // @replayable_gained
    // -----------------------------------------------------------

    const REPLAYABLE_PARENT: &str = r#"
pub extern "c" agent greet() -> Int:
    return 1
"#;
    const REPLAYABLE_COMMIT: &str = r#"
@replayable
pub extern "c" agent greet() -> Int:
    return 1
"#;

    #[test]
    fn replayable_gained_apply_succeeds() {
        let parent = prepare_isolation_input(REPLAYABLE_PARENT).unwrap();
        let commit = prepare_isolation_input(REPLAYABLE_COMMIT).unwrap();
        let splices = compute_splices_for_delta(
            "agent.replayable_gained:greet",
            &parent,
            &commit,
        )
        .expect("isolator produces splices");
        let out = apply_splices(REPLAYABLE_PARENT, splices).expect("splice applies");
        assert!(
            out.contains("@replayable"),
            "synthesized source must contain the added attribute; got:\n{out}"
        );
        assert!(
            out.contains("agent greet"),
            "synthesized source must still contain the agent declaration"
        );
    }

    #[test]
    fn replayable_gained_byte_stability_for_unchanged_regions() {
        // The parent's body text (after the agent header) must
        // appear byte-identical in the synthesized output. The
        // only difference is the inserted `@replayable\n` block.
        let parent = prepare_isolation_input(REPLAYABLE_PARENT).unwrap();
        let commit = prepare_isolation_input(REPLAYABLE_COMMIT).unwrap();
        let splices = compute_splices_for_delta(
            "agent.replayable_gained:greet",
            &parent,
            &commit,
        )
        .unwrap();
        let out = apply_splices(REPLAYABLE_PARENT, splices).unwrap();
        // Parent's body is `pub extern "c" agent greet() -> Int:\n    return 1\n`.
        // That exact byte sequence must survive into the output.
        assert!(
            out.contains("pub extern \"c\" agent greet() -> Int:\n    return 1\n"),
            "parent's agent-body bytes must survive unchanged; got:\n{out}"
        );
    }

    #[test]
    fn replayable_gained_roundtrip_matches_expected_abi_delta() {
        // Confirm the commit genuinely produces a single
        // `agent.replayable_gained:greet` delta (sanity check).
        let parent_abi = compile(REPLAYABLE_PARENT);
        let commit_abi = compile(REPLAYABLE_COMMIT);
        let full_diff = compute_diff_summary(&parent_abi, &commit_abi);
        assert!(
            full_diff
                .records
                .iter()
                .any(|r| r.key == "agent.replayable_gained:greet"),
            "commit must produce the expected delta key; got: {:?}",
            full_diff.records.iter().map(|r| &r.key).collect::<Vec<_>>()
        );
        assert_eq!(full_diff.records.len(), 1, "commit should be a single-delta change");

        // Now apply the isolator to parent and confirm the
        // synthesized source, when diffed against parent,
        // produces exactly that one delta key — no stray side
        // effects, no missed changes.
        let parent_input = prepare_isolation_input(REPLAYABLE_PARENT).unwrap();
        let commit_input = prepare_isolation_input(REPLAYABLE_COMMIT).unwrap();
        let splices = compute_splices_for_delta(
            "agent.replayable_gained:greet",
            &parent_input,
            &commit_input,
        )
        .unwrap();
        let synthesized = apply_splices(REPLAYABLE_PARENT, splices).unwrap();
        let synth_abi = compile(&synthesized);
        let synth_diff = compute_diff_summary(&parent_abi, &synth_abi);
        assert_eq!(
            synth_diff.records.len(),
            1,
            "isolator must produce exactly the target delta; got: {:?}",
            synth_diff.records.iter().map(|r| &r.key).collect::<Vec<_>>()
        );
        assert_eq!(synth_diff.records[0].key, "agent.replayable_gained:greet");
    }

    // -----------------------------------------------------------
    // @replayable_lost (parent has, commit doesn't)
    // -----------------------------------------------------------

    #[test]
    fn replayable_lost_apply_succeeds() {
        // Swap roles: parent has the attribute; commit doesn't.
        let parent_src = REPLAYABLE_COMMIT;
        let commit_src = REPLAYABLE_PARENT;
        let parent = prepare_isolation_input(parent_src).unwrap();
        let commit = prepare_isolation_input(commit_src).unwrap();
        let splices = compute_splices_for_delta(
            "agent.replayable_lost:greet",
            &parent,
            &commit,
        )
        .expect("isolator produces splices");
        let out = apply_splices(parent_src, splices).unwrap();
        assert!(
            !out.contains("@replayable"),
            "synthesized source must not contain the removed attribute; got:\n{out}"
        );
    }

    #[test]
    fn replayable_lost_roundtrip_matches_expected_abi_delta() {
        let parent_src = REPLAYABLE_COMMIT;
        let commit_src = REPLAYABLE_PARENT;
        let parent_abi = compile(parent_src);
        let parent_input = prepare_isolation_input(parent_src).unwrap();
        let commit_input = prepare_isolation_input(commit_src).unwrap();
        let splices = compute_splices_for_delta(
            "agent.replayable_lost:greet",
            &parent_input,
            &commit_input,
        )
        .unwrap();
        let synthesized = apply_splices(parent_src, splices).unwrap();
        let synth_abi = compile(&synthesized);
        let synth_diff = compute_diff_summary(&parent_abi, &synth_abi);
        assert_eq!(synth_diff.records.len(), 1);
        assert_eq!(synth_diff.records[0].key, "agent.replayable_lost:greet");
    }

    // -----------------------------------------------------------
    // Error paths
    // -----------------------------------------------------------

    #[test]
    fn unknown_delta_key_returns_unsupported_class() {
        let parent = prepare_isolation_input(REPLAYABLE_PARENT).unwrap();
        let commit = prepare_isolation_input(REPLAYABLE_COMMIT).unwrap();
        let err = compute_splices_for_delta(
            "agent.something_brand_new:greet",
            &parent,
            &commit,
        )
        .unwrap_err();
        assert!(matches!(err, IsolationError::UnsupportedDeltaClass(_)));
    }
}

