//! Markdown rendering for [`crate::GUARANTEE_REGISTRY`].
//!
//! `render_core_semantics_markdown()` produces the canonical
//! `docs/core-semantics.md` body. The committed file is checked into
//! the repo verbatim; CI fails on drift between the committed text
//! and the live rendering, which keeps spec ≡ implementation.
//!
//! The rendered text is a fixed-format markdown document — no
//! arbitrary timestamps or environment-dependent fields — so the
//! comparison is byte-deterministic and the regen workflow is
//! `corvid contract regen-doc docs/core-semantics.md` followed by
//! a normal commit.

use std::fmt::Write as _;

use crate::{by_kind, GuaranteeClass, GuaranteeKind, GUARANTEE_REGISTRY};

/// Render `docs/core-semantics.md` from the canonical registry.
///
/// The output structure:
///
///   1. Title + intro paragraph + auto-generated banner.
///   2. Summary table: every guarantee row with id / kind / class /
///      phase. Sortable at a glance, scannable in under a minute.
///   3. Detail sections grouped by [`GuaranteeKind`]; each row gets
///      the full description, and `OutOfScope` rows additionally
///      print the `out_of_scope_reason` so the reader sees what
///      Corvid does NOT defend.
///   4. Closing footer pointing at the regen command.
///
/// Returns the full markdown text terminated with a newline.
pub fn render_core_semantics_markdown() -> String {
    let mut out = String::with_capacity(8 * 1024);

    out.push_str("# Corvid core semantics\n\n");
    out.push_str(
        "> Auto-generated from `corvid_guarantees::GUARANTEE_REGISTRY`. \
         **Do not hand-edit.** Update by running\n> \
         `cargo run -q -p corvid-cli -- contract regen-doc docs/core-semantics.md` \
         and committing the result.\n\n",
    );
    out.push_str(
        "Every public Corvid promise about effects, approvals, grounding, \
         budgets, confidence, replay, provenance, and the ABI surface is \
         enumerated below. Each row carries:\n\n",
    );
    out.push_str("- a stable **id** referenced by diagnostics, tests, the bilateral verifier, and `corvid claim --explain`,\n");
    out.push_str("- a **kind** (which moat dimension it belongs to),\n");
    out.push_str(
        "- a **class** — `static` (compiler refuses to produce a binary on \
         violation), `runtime_checked` (runtime detects and surfaces), or \
         `out_of_scope` (a documented promise that does NOT have a check \
         today; reason recorded inline below),\n",
    );
    out.push_str("- the pipeline **phase** that owns the enforcement.\n\n");
    out.push_str(
        "Per the no-shortcuts rule, every `out_of_scope` entry carries an \
         explicit reason. Anything Corvid does not defend appears below \
         in plain language; we do not rely on omission.\n\n",
    );

    // ----------------------------------------------------------------
    // Summary table
    // ----------------------------------------------------------------
    out.push_str("## Summary\n\n");
    out.push_str("| id | kind | class | phase |\n");
    out.push_str("|----|------|-------|-------|\n");
    for g in GUARANTEE_REGISTRY {
        let _ = writeln!(
            out,
            "| `{}` | {} | {} | {} |",
            g.id,
            g.kind.slug(),
            g.class.slug(),
            g.phase.slug()
        );
    }
    out.push('\n');

    // ----------------------------------------------------------------
    // Detail sections, grouped by kind in canonical order.
    // ----------------------------------------------------------------
    out.push_str("## Detail\n\n");
    for kind in GuaranteeKind::ALL {
        let mut entries = by_kind(*kind).peekable();
        if entries.peek().is_none() {
            continue;
        }
        let _ = writeln!(out, "### {}\n", kind_heading(*kind));
        for g in entries {
            let _ = writeln!(out, "#### `{}`", g.id);
            let _ = writeln!(
                out,
                "- **class**: {}\n- **phase**: {}\n",
                g.class.slug(),
                g.phase.slug()
            );
            let _ = writeln!(out, "{}\n", g.description);
            if g.class == GuaranteeClass::OutOfScope && !g.out_of_scope_reason.is_empty() {
                let _ = writeln!(out, "> **Why out of scope:** {}\n", g.out_of_scope_reason);
            }
            if !g.positive_test_refs.is_empty() || !g.adversarial_test_refs.is_empty() {
                if !g.positive_test_refs.is_empty() {
                    out.push_str("**Positive tests:**\n\n");
                    for r in g.positive_test_refs {
                        let _ = writeln!(out, "- `{r}`");
                    }
                    out.push('\n');
                }
                if !g.adversarial_test_refs.is_empty() {
                    out.push_str("**Adversarial tests:**\n\n");
                    for r in g.adversarial_test_refs {
                        let _ = writeln!(out, "- `{r}`");
                    }
                    out.push('\n');
                }
            }
        }
    }

    // ----------------------------------------------------------------
    // Footer
    // ----------------------------------------------------------------
    out.push_str("## Updating this document\n\n");
    out.push_str(
        "This file is generated. To change a description, add a new \
         guarantee, or move an entry between `static` /\n\
         `runtime_checked` / `out_of_scope`, edit \
         `crates/corvid-guarantees/src/lib.rs` and run:\n\n",
    );
    out.push_str("```\n");
    out.push_str("cargo run -q -p corvid-cli -- contract regen-doc docs/core-semantics.md\n");
    out.push_str("```\n\n");
    out.push_str(
        "Then commit the regenerated file together with the registry \
         change. CI fails if the committed text drifts from the \
         registry — there is no quiet way to evolve the spec away from \
         the implementation.\n",
    );
    out
}

fn kind_heading(kind: GuaranteeKind) -> &'static str {
    match kind {
        GuaranteeKind::Approval => "Approval boundaries",
        GuaranteeKind::EffectRow => "Effect rows",
        GuaranteeKind::Grounded => "Grounded provenance",
        GuaranteeKind::Budget => "Budgets",
        GuaranteeKind::Confidence => "Confidence thresholds",
        GuaranteeKind::Replay => "Replay determinism",
        GuaranteeKind::ProvenanceTrace => "Provenance traces",
        GuaranteeKind::AbiDescriptor => "ABI descriptor",
        GuaranteeKind::AbiAttestation => "ABI attestation",
        GuaranteeKind::Server => "Server runtime",
        GuaranteeKind::Jobs => "Durable jobs",
        GuaranteeKind::Auth => "Auth and approvals",
        GuaranteeKind::Connector => "Connectors",
        GuaranteeKind::Observability => "Observability and evals",
        GuaranteeKind::Platform => "Platform — explicit non-defenses",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drift gate: the committed `docs/core-semantics.md` must
    /// equal the live render of [`GUARANTEE_REGISTRY`]. A mismatch
    /// means the registry evolved without re-running the regen
    /// command — fix by `cargo run -q -p corvid-cli -- contract \
    /// regen-doc docs/core-semantics.md` and committing.
    #[test]
    fn rendered_markdown_matches_committed_doc() {
        let committed = include_str!("../../../docs/core-semantics.md");
        let rendered = render_core_semantics_markdown();
        // Tolerate trailing-newline differences but nothing else.
        let committed_norm = committed.trim_end_matches('\n');
        let rendered_norm = rendered.trim_end_matches('\n');
        assert_eq!(
            rendered_norm, committed_norm,
            "docs/core-semantics.md drifted from the registry. \
             Re-run `cargo run -q -p corvid-cli -- contract regen-doc \
             docs/core-semantics.md` and commit."
        );
    }

    #[test]
    fn rendered_markdown_includes_every_registered_id() {
        let rendered = render_core_semantics_markdown();
        for g in GUARANTEE_REGISTRY {
            assert!(
                rendered.contains(g.id),
                "rendered markdown is missing guarantee id `{}`",
                g.id
            );
        }
    }

    #[test]
    fn rendered_markdown_emits_out_of_scope_reasons() {
        let rendered = render_core_semantics_markdown();
        let oos_count = GUARANTEE_REGISTRY
            .iter()
            .filter(|g| g.class == GuaranteeClass::OutOfScope)
            .count();
        let banner_count = rendered.matches("Why out of scope").count();
        assert_eq!(
            banner_count, oos_count,
            "every OutOfScope row must surface its reason in the rendered doc"
        );
    }
}
