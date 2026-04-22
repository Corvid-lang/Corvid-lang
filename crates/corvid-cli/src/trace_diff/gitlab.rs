//! GitLab CI codequality report renderer.
//!
//! Emits a CodeClimate-compatible JSON array that GitLab CI
//! consumes via `artifacts.reports.codequality`. Each delta in
//! the Corvid receipt becomes one codequality issue; GitLab
//! surfaces issues inline on merge-request diffs in the Changes
//! tab and in the MR widget summary.
//!
//! The pipeline pattern:
//!
//! ```yaml
//! corvid_review:
//!   script:
//!     - corvid trace-diff $CI_MERGE_REQUEST_DIFF_BASE_SHA $CI_COMMIT_SHA app.cor --format=gitlab > gl-code-quality-report.json
//!   artifacts:
//!     reports:
//!       codequality: gl-code-quality-report.json
//! ```
//!
//! Severity maps one-to-one from the default policy's verdict:
//! regression-class deltas map to `major` (blocking in strict
//! pipelines); non-regression deltas (added agents, grounded
//! gained, trust raised, etc) map to `info`. The CodeClimate
//! spec's five-level scale
//! (`info | minor | major | critical | blocker`) gives us room
//! to split regressions further in a future slice if needed, but
//! v1 picks `major` for all regressions — it's visible in the MR
//! without being "blocker"-level noise.
//!
//! Fingerprint is the SHA-256 of each delta's canonical `key`.
//! Two runs on the same PR produce byte-identical fingerprints,
//! so GitLab correctly dedupes the MR widget's issue list across
//! pipeline re-runs.

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::receipt::{Receipt, Verdict};

/// CodeClimate/GitLab severity scale. Ordered
/// `info < minor < major < critical < blocker`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum GitlabSeverity {
    Info,
    #[allow(dead_code)]
    Minor,
    Major,
    #[allow(dead_code)]
    Critical,
    #[allow(dead_code)]
    Blocker,
}

#[derive(Debug, Clone, Serialize)]
struct GitlabIssue {
    description: String,
    check_name: &'static str,
    fingerprint: String,
    severity: GitlabSeverity,
    location: GitlabLocation,
}

#[derive(Debug, Clone, Serialize)]
struct GitlabLocation {
    path: String,
    lines: GitlabLines,
}

#[derive(Debug, Clone, Serialize)]
struct GitlabLines {
    begin: u32,
}

/// Every Corvid issue uses this check_name so GitLab groups
/// findings cleanly in the MR widget. Stable across runs.
const CHECK_NAME: &str = "corvid.trace-diff";

/// Compute the fingerprint for a delta: hex-SHA256 of the
/// delta key. Stable across runs of the same PR so GitLab
/// dedupes issues across pipeline re-runs.
fn fingerprint_for(delta_key: &str) -> String {
    let mut h = Sha256::new();
    h.update(delta_key.as_bytes());
    hex::encode(h.finalize())
}

/// Is this delta a policy regression that would fail the gate?
/// The default-policy flag set is embedded in the verdict's
/// `flags` field; we match by checking whether the delta's
/// summary appears there. This keeps severity in sync with
/// whatever policy is in effect — custom policies (future
/// `-custom-policy` slice) will flow through the same check
/// automatically.
fn is_regression_delta(delta_key: &str, delta_summary: &str, verdict: &Verdict) -> bool {
    // Summary-based match is robust against key-format
    // refactors — the policy emits flags keyed by summary, not
    // delta_key. A prefix scan catches both direct flag
    // occurrences and anchored matches (e.g., `agent
    // `refund_bot` became `@dangerous`` appears in
    // verdict.flags after the default policy fires).
    let _ = delta_key;
    verdict.flags.iter().any(|flag| flag.contains(delta_summary))
}

fn render_issues(receipt: &Receipt, verdict: &Verdict) -> Vec<GitlabIssue> {
    let mut out = Vec::with_capacity(receipt.deltas.len());
    for delta in &receipt.deltas {
        let severity = if is_regression_delta(&delta.key, &delta.summary, verdict) {
            GitlabSeverity::Major
        } else {
            GitlabSeverity::Info
        };
        out.push(GitlabIssue {
            description: delta.summary.clone(),
            check_name: CHECK_NAME,
            fingerprint: fingerprint_for(&delta.key),
            severity,
            location: GitlabLocation {
                path: receipt.source_path.clone(),
                // Line numbers aren't in the receipt today —
                // deltas are span-less summaries of algebraic
                // changes. Line 1 is GitLab's accepted default
                // when a finding isn't pinned to a specific
                // location. A future slice can enrich deltas
                // with source spans from the ABI descriptor.
                lines: GitlabLines { begin: 1 },
            },
        });
    }

    // Counterfactual-replay trace-impact becomes an issue too,
    // so the MR widget surfaces trace regressions as first-class
    // findings — not just schema-delta ones. Stable fingerprint
    // keeps the issue deduped across pipeline reruns.
    if receipt.impact.any_newly_diverged {
        out.push(GitlabIssue {
            description: format!(
                "{} (counterfactual replay)",
                receipt.impact.summary_line.trim()
            ),
            check_name: CHECK_NAME,
            fingerprint: fingerprint_for("corvid.trace-impact.newly-diverged"),
            severity: GitlabSeverity::Major,
            location: GitlabLocation {
                path: receipt.source_path.clone(),
                lines: GitlabLines { begin: 1 },
            },
        });
    }

    out
}

/// Render the receipt as a GitLab codequality JSON array with a
/// trailing newline (every format ends with `\n` for shell-
/// pipeline consistency).
pub(super) fn render_gitlab(receipt: &Receipt, verdict: &Verdict) -> String {
    let issues = render_issues(receipt, verdict);
    let mut s = serde_json::to_string_pretty(&issues)
        .expect("GitLab codequality issues are trivially serializable");
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace_diff::impact::TraceImpact;
    use crate::trace_diff::narrative::{DeltaRecord, ReceiptNarrative};
    use crate::trace_diff::receipt::RECEIPT_SCHEMA_VERSION;

    fn delta(key: &str, summary: &str) -> DeltaRecord {
        DeltaRecord {
            key: key.to_string(),
            summary: summary.to_string(),
        }
    }

    fn receipt_with(deltas: Vec<DeltaRecord>, impact: TraceImpact) -> Receipt {
        Receipt {
            schema_version: RECEIPT_SCHEMA_VERSION,
            base_sha: "base".into(),
            head_sha: "head".into(),
            source_path: "src/agent.cor".into(),
            deltas,
            impact,
            narrative: ReceiptNarrative::empty(),
            narrative_rejected: false,
        }
    }

    #[test]
    fn fingerprint_is_deterministic_sha256_hex() {
        let a = fingerprint_for("agent.added:refund_bot");
        let b = fingerprint_for("agent.added:refund_bot");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn fingerprint_differs_by_delta_key() {
        let a = fingerprint_for("agent.added:foo");
        let b = fingerprint_for("agent.added:bar");
        assert_ne!(a, b);
    }

    #[test]
    fn non_regression_delta_is_info_severity() {
        let verdict = Verdict {
            ok: true,
            flags: vec![],
        };
        let receipt = receipt_with(
            vec![delta("agent.added:foo", "new agent `foo`")],
            TraceImpact::empty(),
        );
        let issues = render_issues(&receipt, &verdict);
        assert_eq!(issues.len(), 1);
        assert!(matches!(issues[0].severity, GitlabSeverity::Info));
    }

    #[test]
    fn regression_delta_is_major_severity() {
        // Policy flagged this summary — severity should reflect
        // that.
        let verdict = Verdict {
            ok: false,
            flags: vec![
                "agent `refund_bot` became `@dangerous`".into(),
            ],
        };
        let receipt = receipt_with(
            vec![delta(
                "agent.dangerous_gained:refund_bot",
                "agent `refund_bot` became `@dangerous`",
            )],
            TraceImpact::empty(),
        );
        let issues = render_issues(&receipt, &verdict);
        assert_eq!(issues.len(), 1);
        assert!(matches!(issues[0].severity, GitlabSeverity::Major));
    }

    #[test]
    fn every_issue_has_stable_check_name() {
        let verdict = Verdict {
            ok: true,
            flags: vec![],
        };
        let receipt = receipt_with(
            vec![
                delta("agent.added:foo", "new agent `foo`"),
                delta("agent.added:bar", "new agent `bar`"),
            ],
            TraceImpact::empty(),
        );
        let issues = render_issues(&receipt, &verdict);
        assert!(issues.iter().all(|i| i.check_name == "corvid.trace-diff"));
    }

    #[test]
    fn trace_impact_becomes_a_major_issue_when_diverged() {
        let verdict = Verdict {
            ok: false,
            flags: vec!["counterfactual replay: ...".into()],
        };
        let mut impact = TraceImpact::empty();
        impact.has_traces = true;
        impact.any_newly_diverged = true;
        impact.summary_line = "Replayed 3 trace(s): 2 newly diverged.".into();
        let receipt = receipt_with(vec![], impact);
        let issues = render_issues(&receipt, &verdict);
        assert_eq!(issues.len(), 1);
        assert!(issues[0].description.contains("counterfactual replay"));
        assert!(matches!(issues[0].severity, GitlabSeverity::Major));
    }

    #[test]
    fn empty_receipt_renders_empty_array() {
        let verdict = Verdict {
            ok: true,
            flags: vec![],
        };
        let receipt = receipt_with(vec![], TraceImpact::empty());
        let out = render_gitlab(&receipt, &verdict);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(parsed.is_array());
        assert_eq!(parsed.as_array().unwrap().len(), 0);
    }

    #[test]
    fn output_is_valid_codequality_shape() {
        let verdict = Verdict {
            ok: false,
            flags: vec!["x".into()],
        };
        let receipt = receipt_with(
            vec![delta("agent.added:foo", "new agent `foo`")],
            TraceImpact::empty(),
        );
        let out = render_gitlab(&receipt, &verdict);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        let issue = &parsed[0];
        // Every codequality issue needs these fields per the
        // CodeClimate spec GitLab consumes.
        assert!(issue["description"].is_string());
        assert!(issue["check_name"].is_string());
        assert!(issue["fingerprint"].is_string());
        assert!(issue["severity"].is_string());
        assert!(issue["location"]["path"].is_string());
        assert!(issue["location"]["lines"]["begin"].is_number());
    }

    #[test]
    fn fingerprint_stays_stable_across_runs() {
        // GitLab dedupes MR widget issues by fingerprint. If
        // fingerprints drift between CI runs, the widget shows
        // phantom "new" findings on every re-run. Regression
        // guard.
        let verdict = Verdict {
            ok: true,
            flags: vec![],
        };
        let receipt = receipt_with(
            vec![delta("agent.added:foo", "new agent `foo`")],
            TraceImpact::empty(),
        );
        let run1 = render_gitlab(&receipt, &verdict);
        let run2 = render_gitlab(&receipt, &verdict);
        assert_eq!(run1, run2, "byte-identical across runs");
    }
}
