//! Shared AST-traversal helpers + a probe-round-trip utility for
//! per-class isolators in `delta_isolation`.
//!
//! Extracted from `annotations.rs` once a second consumer
//! (`structural.rs`) needed `find_agent` — one-extraction-per-
//! commit discipline. The probe-round-trip helper is new: every
//! per-class isolator uses it to *verify* the ABI diff between
//! parent and commit sources actually contains the delta key the
//! class claims to isolate, before any three-test-triad round-
//! trip test runs. If the probe fails, the class is deferred
//! (per the trust-tier discipline), not soft-shipped.
//!
//! Keeping this module free of `corvid-driver`-specific logic in
//! the pure AST helpers and constraining the probe helper to the
//! one function that does need the compiler.

use corvid_ast::{AgentDecl, Decl, File};

use super::super::narrative::compute_diff_summary;

/// Find the first agent declaration with the given name. Returns
/// `None` when no top-level `agent <name>` declaration exists —
/// isolators treat this as a missing-from-expected-side error.
pub(super) fn find_agent<'a>(file: &'a File, name: &str) -> Option<&'a AgentDecl> {
    file.decls.iter().find_map(|decl| match decl {
        Decl::Agent(agent) if agent.name.name == name => Some(agent),
        _ => None,
    })
}

/// Compile `parent_src` and `commit_src` to ABIs, diff them, and
/// return whether the diff contains `expected_delta_key`. Used by
/// per-class isolator tests to confirm — *before* running the
/// three-test triad — that the delta key the class claims to
/// isolate actually fires on the test fixture. Catches cases
/// where an annotation or body change doesn't actually
/// ABI-compose (the exact thing that caught trust-tier during
/// `3c-annotations`).
///
/// Panics on compile failure — probes run against curated
/// fixtures; compile failure means the fixture is malformed
/// and the test author needs to fix it.
#[cfg(test)]
pub(super) fn probe_round_trip(
    parent_src: &str,
    commit_src: &str,
    expected_delta_key: &str,
) -> bool {
    use corvid_driver::compile_to_abi_with_config;
    const GENERATED_AT: &str = "1970-01-01T00:00:00Z";

    let parent_abi = compile_to_abi_with_config(parent_src, "test.cor", GENERATED_AT, None)
        .unwrap_or_else(|diags| panic!("parent compile failed: {} diag(s)", diags.len()));
    let commit_abi = compile_to_abi_with_config(commit_src, "test.cor", GENERATED_AT, None)
        .unwrap_or_else(|diags| panic!("commit compile failed: {} diag(s)", diags.len()));
    let diff = compute_diff_summary(&parent_abi, &commit_abi);
    diff.records.iter().any(|r| r.key == expected_delta_key)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn find_agent_locates_named_declaration() {
        let (file, errs) =
            corvid_syntax::parse_file(&corvid_syntax::lex(REPLAYABLE_PARENT).unwrap());
        assert!(errs.is_empty());
        let found = find_agent(&file, "greet");
        assert!(found.is_some());
    }

    #[test]
    fn find_agent_returns_none_on_unknown_name() {
        let (file, errs) =
            corvid_syntax::parse_file(&corvid_syntax::lex(REPLAYABLE_PARENT).unwrap());
        assert!(errs.is_empty());
        assert!(find_agent(&file, "not_an_agent").is_none());
    }

    #[test]
    fn probe_round_trip_confirms_replayable_delta_fires() {
        // Sanity-check the probe helper against a class we
        // already know round-trips — @replayable. If this test
        // regresses, the probe helper is broken before any
        // class-specific test is run against it.
        assert!(probe_round_trip(
            REPLAYABLE_PARENT,
            REPLAYABLE_COMMIT,
            "agent.replayable_gained:greet",
        ));
    }

    #[test]
    fn probe_round_trip_returns_false_when_expected_key_absent() {
        // Same sources, wrong expected key — probe must say no.
        assert!(!probe_round_trip(
            REPLAYABLE_PARENT,
            REPLAYABLE_COMMIT,
            "agent.dangerous_gained:greet",
        ));
    }
}
