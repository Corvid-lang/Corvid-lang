//! The canonical structured receipt plus everything downstream of
//! it: output-format selection + rendering, and the verdict an
//! exit code reads from.
//!
//! `Receipt` is the one source of truth. Each format (`markdown`,
//! `github-check`, `json`) is a view over the same struct. Markdown
//! stays Corvid-side — the reviewer agent owns receipt layout for
//! humans and that's the load-bearing dogfood of the slice. The
//! other renderers live here in Rust because they're CI plumbing
//! (GitHub annotation commands) or serde boilerplate (schema-
//! versioned JSON) — putting them in Corvid would be ceremony
//! without payoff until the language has better string / JSON
//! support.
//!
use std::io::IsTerminal;

use corvid_abi::CorvidAbi;
use serde::{Deserialize, Serialize};

use super::impact::TraceImpact;
use super::narrative::{compute_diff_summary, DeltaRecord, DiffSummary, ReceiptNarrative};

/// Schema version embedded into the JSON renderer's output. Bump
/// whenever any shipped field's meaning or shape changes so bots
/// can pin. Additive field additions are NOT a version bump —
/// they're backward-compatible by construction.
///
/// v2 (2026-04-22): rename the misleadingly-named delta keys
/// `agent.approval.tier_weakened` → `agent.approval.tier_changed`
/// and `agent.approval.reversibility_weakened` →
/// `agent.approval.reversibility_changed`. Both keys emit on any
/// transition (weakening OR strengthening) — the old names lied
/// about what they represented. The policy layer still gates only
/// on weakenings by parsing the `from->to` suffix. Consumers
/// pattern-matching on the old prefixes must update their matchers.
pub(super) const RECEIPT_SCHEMA_VERSION: u32 = 2;

/// Output format for the receipt. Each mode is a renderer over
/// the same canonical [`Receipt`]; adding a new mode is a new
/// renderer + a new enum variant, never a pipeline change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// Human-readable markdown via the Corvid reviewer. Default
    /// when stdout is a tty and no CI environment is detected.
    Markdown,
    /// GitHub Actions annotation commands (`::notice`,
    /// `::warning`) on stdout; the GHA runner captures these
    /// into PR check annotations. Default under `$GITHUB_ACTIONS`.
    GithubCheck,
    /// Schema-versioned structured JSON. Default when stdout is
    /// piped — bots usually want this. Explicit opt-in via
    /// `--format=json`.
    Json,
    /// in-toto Statement v1 with the Corvid receipt as the
    /// predicate. When combined with `--sign`, the DSSE envelope
    /// uses `application/vnd.in-toto+json` as its payloadType so
    /// cosign / attest-tools / slsa-verifier consume the output
    /// natively. Explicit opt-in via `--format=in-toto`.
    InToto,
    /// GitLab CI codequality report JSON (CodeClimate-compatible).
    /// Array of issue objects that GitLab picks up via
    /// `artifacts.reports.codequality` and surfaces inline on MR
    /// diffs. Explicit opt-in via `--format=gitlab`. Each delta
    /// in the receipt becomes one issue; severity follows the
    /// policy (regressions = `major`, informational = `info`).
    Gitlab,
}

impl OutputFormat {
    /// Parse `--format=<mode>` accepting the five user-facing
    /// spellings: `auto` (environment-driven), `markdown`,
    /// `github-check`, `json`, `in-toto`. Rejects other values
    /// with a typed error so the caller can surface guidance.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "auto" => Ok(Self::detect_from_environment()),
            "markdown" => Ok(Self::Markdown),
            "github-check" => Ok(Self::GithubCheck),
            "json" => Ok(Self::Json),
            "in-toto" => Ok(Self::InToto),
            "gitlab" => Ok(Self::Gitlab),
            other => Err(format!(
                "unknown format `{other}`; expected `auto`, `markdown`, `github-check`, `json`, `in-toto`, or `gitlab`"
            )),
        }
    }

    /// Default-pick based on environment. Priority: explicit CI
    /// platform env var > stdout-is-pipe > tty. Feels magical but
    /// matches what any CI-aware CLI should do by default.
    fn detect_from_environment() -> Self {
        if std::env::var_os("GITHUB_ACTIONS").is_some() {
            return Self::GithubCheck;
        }
        if std::env::var_os("GITLAB_CI").is_some() {
            return Self::Gitlab;
        }
        if !std::io::stdout().is_terminal() {
            return Self::Json;
        }
        Self::Markdown
    }
}

/// The canonical structured receipt. Source of truth for every
/// renderer and for the policy engine. `schema_version` is a
/// promise to downstream JSON consumers; the field lives here
/// rather than only in the JSON encoding so the CLI can emit it
/// consistently across formats (e.g. as metadata in a signed
/// envelope).
#[derive(Debug, Clone, Serialize)]
pub(super) struct Receipt {
    pub schema_version: u32,
    pub base_sha: String,
    pub head_sha: String,
    pub source_path: String,
    pub deltas: Vec<DeltaRecord>,
    pub impact: TraceImpact,
    pub narrative: ReceiptNarrative,
    pub narrative_rejected: bool,
}

impl Receipt {
    pub(super) fn build(
        base_sha: &str,
        head_sha: &str,
        source_path: &str,
        base_abi: &CorvidAbi,
        head_abi: &CorvidAbi,
        impact: TraceImpact,
        narrative: ReceiptNarrative,
        narrative_rejected: bool,
    ) -> Self {
        let DiffSummary { records } = compute_diff_summary(base_abi, head_abi);
        Self {
            schema_version: RECEIPT_SCHEMA_VERSION,
            base_sha: base_sha.to_string(),
            head_sha: head_sha.to_string(),
            source_path: source_path.to_string(),
            deltas: records,
            impact,
            narrative,
            narrative_rejected,
        }
    }
}

/// Verdict from the regression policy. `flags` carries a
/// human-readable line per failed gate; callers emit them to
/// stderr on non-`ok` verdicts and the JSON renderer folds them
/// into `regression_flags`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Verdict {
    pub ok: bool,
    pub flags: Vec<String>,
}

/// Conservative default regression policy. Trips on the safety-
/// lowering set agreed during the pre-phase chat: `@dangerous`
/// gained, trust-tier lowered, approval required-tier lowered,
/// reversibility became `irreversible`, grounded attestation
/// lost, any newly-diverged trace under counterfactual replay.
///
/// Everything else — additions, removals, improvements — is
/// informational and does NOT flip the verdict. This matches the
/// "flag regressions, celebrate improvements" asymmetry. Custom
/// policies (in `21-inv-H-5-custom-policy`) replace this via
/// user-supplied `.cor` programs.
#[cfg(test)]
pub(super) fn apply_default_policy(receipt: &Receipt) -> Verdict {
    let mut flags = Vec::new();

    for delta in &receipt.deltas {
        if let Some(flag) = regression_flag_for(delta) {
            flags.push(flag);
        }
    }

    if receipt.impact.any_newly_diverged {
        flags.push(format!(
            "counterfactual replay: {}",
            receipt.impact.summary_line
        ));
    }

    Verdict {
        ok: flags.is_empty(),
        flags,
    }
}

/// Classify one delta against the default policy. Returns
/// `Some(flag_string)` if the delta represents a safety
/// regression, `None` otherwise. Trust-tier and approval-tier
/// keys encode the transition as `...:<from>-><to>` so we can
/// check direction without parsing ordinals on the Rust side —
/// the record carries the comparison.
pub(super) fn regression_flag_for(delta: &DeltaRecord) -> Option<String> {
    let key = delta.key.as_str();
    if key.starts_with("agent.dangerous_gained:") {
        return Some(delta.summary.clone());
    }
    if key.starts_with("agent.provenance.grounded_lost:") {
        return Some(delta.summary.clone());
    }
    if key.starts_with("agent.provenance.dep_removed:") {
        return Some(delta.summary.clone());
    }
    if key.starts_with("agent.trust_tier_changed:") {
        if let Some(transition) = key.rsplit(':').next() {
            if is_trust_lowering(transition) {
                return Some(delta.summary.clone());
            }
        }
    }
    if key.starts_with("agent.approval.tier_changed:") {
        // The key fires on any transition; the policy trips only
        // when the transition lowers trust. Strengthenings flow
        // through the receipt as info-level deltas without
        // flipping the gate.
        if let Some(transition) = key.rsplit(':').next() {
            if is_trust_lowering(transition) {
                return Some(delta.summary.clone());
            }
        }
    }
    if key.starts_with("agent.approval.reversibility_changed:") {
        // Same shape: all reversibility transitions emit, only
        // `*->irreversible` trips the gate.
        if let Some(transition) = key.rsplit(':').next() {
            if transition.ends_with("->irreversible") {
                return Some(delta.summary.clone());
            }
        }
    }
    if key.starts_with("agent.extern.ownership_changed:") {
        if let Some(transition) = key.rsplit(':').next() {
            if is_ownership_loosening(transition) {
                return Some(delta.summary.clone());
            }
        }
    }
    None
}

/// Trust-tier ordering for regression detection. Lower ordinal
/// = more permissive / less safe. An `a->b` transition is a
/// lowering when `b`'s ordinal is less than `a`'s. Unknown tiers
/// compare as middle-of-the-road and don't flip the gate.
pub(super) fn is_trust_lowering(transition: &str) -> bool {
    let Some((from, to)) = transition.split_once("->") else {
        return false;
    };
    let Some(from_rank) = tier_ordinal(from) else {
        return false;
    };
    let Some(to_rank) = tier_ordinal(to) else {
        return false;
    };
    to_rank < from_rank
}

/// Ordinal for Corvid's built-in trust tiers. Higher = stricter.
/// Matches the checker's tier lattice; if a new tier lands in
/// `corvid-types::dimensions` this table must be updated, which
/// is what the tier-drift guard test in `effect_filter.rs`
/// protects against on Dev B's side. The unit test
/// `tier_ordering_matches_policy` below is the corresponding
/// backstop on our side.
fn tier_ordinal(tier: &str) -> Option<u8> {
    match tier {
        "autonomous" => Some(0),
        "human_supervised" => Some(1),
        "human_required" => Some(2),
        _ => None,
    }
}

pub(super) fn is_ownership_loosening(transition: &str) -> bool {
    let Some((from, to)) = transition.split_once("->") else {
        return false;
    };
    !from.starts_with("@borrowed") && to.starts_with("@borrowed")
}

/// Render a receipt in the GitHub Actions annotation format.
/// Prints one `::notice` / `::warning` command per meaningful
/// delta on stdout; the GHA runner promotes these into PR check
/// annotations. Narrative (if present) is emitted as a single
/// `::notice title=PR Behavior`. Regression flags from the policy
/// are emitted as `::warning` so they surface as annotations
/// on top of the non-zero exit code.
pub(super) fn render_github_check(receipt: &Receipt, verdict: &Verdict) -> String {
    let mut out = String::new();

    if !receipt.narrative.body.is_empty() {
        out.push_str(&format!(
            "::notice title=PR Behavior Summary::{}\n",
            escape_gha_message(&receipt.narrative.body)
        ));
    }

    for flag in &verdict.flags {
        out.push_str(&format!(
            "::warning title=Regression::{}\n",
            escape_gha_message(flag)
        ));
    }

    for delta in &receipt.deltas {
        if verdict.flags.contains(&delta.summary) {
            // already surfaced above as a warning, don't duplicate
            continue;
        }
        out.push_str(&format!(
            "::notice title={}::{}\n",
            escape_gha_title(&delta.key),
            escape_gha_message(&delta.summary)
        ));
    }

    if receipt.impact.has_traces {
        out.push_str(&format!(
            "::notice title=Counterfactual Replay::{}\n",
            escape_gha_message(&receipt.impact.summary_line)
        ));
    }

    out
}

/// GHA command format reserves `%`, `\r`, `\n` and `:` / `,` in
/// specific positions. The conservative encoding replaces them
/// all; worst case we produce slightly less readable annotations
/// on pathological inputs, but we never corrupt the command
/// stream. Source: GitHub Actions runner docs.
fn escape_gha_message(s: &str) -> String {
    s.replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

fn escape_gha_title(s: &str) -> String {
    escape_gha_message(s).replace(':', "%3A").replace(',', "%2C")
}

/// Render a receipt as schema-versioned JSON. The top-level
/// shape is:
///
/// ```json
/// {
///   "schema_version": 2,
///   "base_sha": "...",
///   "head_sha": "...",
///   "source_path": "...",
///   "verdict": { "ok": true, "flags": [] },
///   "receipt": {
///     "deltas": [...],
///     "impact": { ... },
///     "narrative": { ... },
///     "narrative_rejected": false
///   }
/// }
/// ```
///
/// JSON output is newline-terminated and stable-sorted at the
/// field level via serde's struct-field ordering. Bots that
/// want to hash the output for caching can do so directly.
pub(super) fn render_json(receipt: &Receipt, verdict: &Verdict) -> String {
    let envelope = serde_json::json!({
        "schema_version": receipt.schema_version,
        "base_sha": receipt.base_sha,
        "head_sha": receipt.head_sha,
        "source_path": receipt.source_path,
        "verdict": verdict,
        "receipt": {
            "deltas": receipt.deltas,
            "impact": receipt.impact,
            "narrative": receipt.narrative,
            "narrative_rejected": receipt.narrative_rejected,
        }
    });
    // serde_json::to_string_pretty keeps human readability; bots
    // that want a single-line form can parse + re-emit.
    let mut out =
        serde_json::to_string_pretty(&envelope).expect("Receipt JSON envelope is serializable");
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace_diff::narrative::DeltaCitation;

    fn delta(key: &str, summary: &str) -> DeltaRecord {
        DeltaRecord {
            key: key.to_string(),
            summary: summary.to_string(),
        }
    }

    fn empty_impact() -> TraceImpact {
        TraceImpact::empty()
    }

    fn diverged_impact(count: usize) -> TraceImpact {
        TraceImpact {
            has_traces: true,
            any_newly_diverged: count > 0,
            summary_line: format!("Replayed 10 trace(s): {count} newly diverged."),
            impact_percentage: format!("{}.0%", count * 10),
            newly_diverged_paths: (0..count).map(|i| format!("t{i}.jsonl")).collect(),
        }
    }

    fn empty_narrative() -> ReceiptNarrative {
        ReceiptNarrative::empty()
    }

    fn receipt_with_deltas(deltas: Vec<DeltaRecord>, impact: TraceImpact) -> Receipt {
        Receipt {
            schema_version: RECEIPT_SCHEMA_VERSION,
            base_sha: "base".into(),
            head_sha: "head".into(),
            source_path: "app.cor".into(),
            deltas,
            impact,
            narrative: empty_narrative(),
            narrative_rejected: false,
        }
    }

    #[test]
    fn format_parses_from_str() {
        // `auto` is environment-dependent — assert only that it
        // resolves to one of the concrete formats, not a specific
        // one, since the test env may or may not have
        // `GITHUB_ACTIONS` set.
        assert!(matches!(
            OutputFormat::parse("auto").unwrap(),
            OutputFormat::Markdown
                | OutputFormat::GithubCheck
                | OutputFormat::Json
                | OutputFormat::Gitlab
        ));
        assert_eq!(
            OutputFormat::parse("markdown").unwrap(),
            OutputFormat::Markdown
        );
        assert_eq!(
            OutputFormat::parse("github-check").unwrap(),
            OutputFormat::GithubCheck
        );
        assert_eq!(OutputFormat::parse("json").unwrap(), OutputFormat::Json);
        assert_eq!(OutputFormat::parse("in-toto").unwrap(), OutputFormat::InToto);
        assert_eq!(OutputFormat::parse("gitlab").unwrap(), OutputFormat::Gitlab);
        assert!(OutputFormat::parse("wat").is_err());
    }

    #[test]
    fn default_policy_passes_on_empty_receipt() {
        let r = receipt_with_deltas(vec![], empty_impact());
        let v = apply_default_policy(&r);
        assert!(v.ok);
        assert!(v.flags.is_empty());
    }

    #[test]
    fn default_policy_flags_dangerous_gained() {
        let r = receipt_with_deltas(
            vec![delta(
                "agent.dangerous_gained:refund_bot",
                "agent `refund_bot` became `@dangerous`",
            )],
            empty_impact(),
        );
        let v = apply_default_policy(&r);
        assert!(!v.ok);
        assert_eq!(v.flags.len(), 1);
        assert!(v.flags[0].contains("became `@dangerous`"));
    }

    #[test]
    fn default_policy_flags_trust_lowering_only() {
        // human_required -> autonomous is a lowering → flagged.
        let lowered = receipt_with_deltas(
            vec![delta(
                "agent.trust_tier_changed:refund_bot:human_required->autonomous",
                "agent `refund_bot` trust-tier changed from `human_required` to `autonomous`",
            )],
            empty_impact(),
        );
        assert!(!apply_default_policy(&lowered).ok);

        // autonomous -> human_required is a raising → NOT flagged.
        let raised = receipt_with_deltas(
            vec![delta(
                "agent.trust_tier_changed:refund_bot:autonomous->human_required",
                "agent `refund_bot` trust-tier changed from `autonomous` to `human_required`",
            )],
            empty_impact(),
        );
        assert!(apply_default_policy(&raised).ok);
    }

    #[test]
    fn default_policy_flags_reversibility_becoming_irreversible() {
        let r = receipt_with_deltas(
            vec![delta(
                "agent.approval.reversibility_changed:refund_bot:IssueRefund:reversible->irreversible",
                "reversibility regression on IssueRefund",
            )],
            empty_impact(),
        );
        assert!(!apply_default_policy(&r).ok);
    }

    #[test]
    fn default_policy_flags_grounded_lost() {
        let r = receipt_with_deltas(
            vec![delta(
                "agent.provenance.grounded_lost:answer_question",
                "answer_question return value lost Grounded<T> provenance",
            )],
            empty_impact(),
        );
        assert!(!apply_default_policy(&r).ok);
    }

    #[test]
    fn default_policy_flags_newly_diverged_traces() {
        let r = receipt_with_deltas(vec![], diverged_impact(3));
        let v = apply_default_policy(&r);
        assert!(!v.ok);
        assert!(v.flags[0].contains("newly diverged"));
    }

    #[test]
    fn default_policy_ignores_improvements() {
        // All of these are "good" changes; the policy should pass.
        let r = receipt_with_deltas(
            vec![
                delta("agent.added:foo", "new agent `foo`"),
                delta(
                    "agent.trust_tier_changed:foo:autonomous->human_required",
                    "trust raised",
                ),
                delta("agent.provenance.grounded_gained:foo", "gained Grounded<T>"),
                delta("agent.approval.label_added:foo:Bar", "approve site added"),
                delta("agent.dangerous_lost:foo", "no longer @dangerous"),
            ],
            empty_impact(),
        );
        assert!(apply_default_policy(&r).ok);
    }

    #[test]
    fn tier_ordering_matches_policy() {
        // Backstop against silent drift if `corvid-types` adds a
        // tier — if this fails, `tier_ordinal` needs updating and
        // we should also revisit whether any existing policy
        // flag logic relies on the old tier set. Mirror of the
        // tier-drift guard test on the 22-D effect-filter side.
        assert!(is_trust_lowering("human_required->autonomous"));
        assert!(is_trust_lowering("human_required->human_supervised"));
        assert!(is_trust_lowering("human_supervised->autonomous"));
        assert!(!is_trust_lowering("autonomous->human_required"));
        assert!(!is_trust_lowering("human_supervised->human_required"));
        assert!(!is_trust_lowering("autonomous->autonomous"));
    }

    #[test]
    fn render_json_has_schema_version() {
        let r = receipt_with_deltas(vec![delta("agent.added:foo", "new")], empty_impact());
        let v = apply_default_policy(&r);
        let out = render_json(&r, &v);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["schema_version"], 2);
        assert_eq!(parsed["verdict"]["ok"], true);
        assert_eq!(parsed["receipt"]["deltas"][0]["key"], "agent.added:foo");
    }

    #[test]
    fn render_json_surfaces_regression_flags() {
        let r = receipt_with_deltas(
            vec![delta(
                "agent.dangerous_gained:refund_bot",
                "refund_bot became @dangerous",
            )],
            empty_impact(),
        );
        let v = apply_default_policy(&r);
        let out = render_json(&r, &v);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["verdict"]["ok"], false);
        let flags = parsed["verdict"]["flags"].as_array().unwrap();
        assert_eq!(flags.len(), 1);
        assert!(flags[0].as_str().unwrap().contains("@dangerous"));
    }

    #[test]
    fn render_github_check_emits_annotation_commands() {
        let r = receipt_with_deltas(
            vec![
                delta("agent.added:foo", "new agent `foo`"),
                delta(
                    "agent.dangerous_gained:refund_bot",
                    "agent `refund_bot` became `@dangerous`",
                ),
            ],
            empty_impact(),
        );
        let v = apply_default_policy(&r);
        let out = render_github_check(&r, &v);

        // Regression gets a `::warning` line.
        assert!(
            out.contains("::warning title=Regression::agent `refund_bot` became `@dangerous`"),
            "got:\n{out}"
        );
        // Non-regression delta gets a `::notice` line.
        assert!(
            out.contains("::notice title=agent.added%3Afoo::new agent `foo`"),
            "got:\n{out}"
        );
        // Regression-shaped delta is NOT also emitted as a duplicate
        // notice (dedupe check).
        assert_eq!(
            out.matches("became `@dangerous`").count(),
            1,
            "got:\n{out}"
        );
    }

    #[test]
    fn render_github_check_escapes_newlines_in_messages() {
        let r = receipt_with_deltas(
            vec![delta(
                "agent.added:multi",
                "summary with\nnewline and %percent",
            )],
            empty_impact(),
        );
        let v = apply_default_policy(&r);
        let out = render_github_check(&r, &v);
        assert!(out.contains("%0A"), "newline must be escaped: {out}");
        assert!(out.contains("%25"), "% must be escaped: {out}");
        // No raw literal newline in the message part of the annotation.
        for line in out.lines() {
            assert!(!line.is_empty() || out.lines().last().unwrap() == line);
        }
    }

    #[test]
    fn render_github_check_with_narrative_emits_header_notice() {
        let mut r = receipt_with_deltas(vec![], empty_impact());
        r.narrative = ReceiptNarrative {
            body: "refund_bot gained @dangerous.".to_string(),
            citations: vec![DeltaCitation {
                delta_key: "agent.dangerous_gained:refund_bot".to_string(),
            }],
        };
        let v = apply_default_policy(&r);
        let out = render_github_check(&r, &v);
        assert!(
            out.starts_with("::notice title=PR Behavior Summary::refund_bot gained @dangerous."),
            "got:\n{out}"
        );
    }
}
