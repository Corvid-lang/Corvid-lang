//! in-toto Statement v1 renderer for Corvid receipts.
//!
//! Wraps the canonical [`Receipt`](super::receipt::Receipt) in an
//! in-toto Statement v1 envelope with the receipt as the predicate.
//! When combined with `--sign`, the DSSE envelope uses
//! `application/vnd.in-toto+json` as its payloadType (the in-toto
//! convention) rather than Corvid's native
//! `application/vnd.corvid-receipt+json` — so cosign,
//! slsa-verifier, attest-tools, and the rest of the in-toto
//! ecosystem consume the output natively.
//!
//! Statement shape (v1):
//!
//! ```json
//! {
//!   "_type": "https://in-toto.io/Statement/v1",
//!   "subject": [
//!     {
//!       "name": "<source-path-in-repo>",
//!       "digest": { "sha256": "<head-source-hash>" }
//!     }
//!   ],
//!   "predicateType": "https://corvid-lang.org/attestation/receipt/v1",
//!   "predicate": { /* the full H-5 Receipt */ }
//! }
//! ```
//!
//! Design commitments (named here so future slice authors
//! inherit them):
//!
//! - **Subject = reviewed artifact, not the receipt itself.** The
//!   attestation is *about* the head source file, not about the
//!   receipt that describes its review. Self-attesting would be
//!   redundant with the signed-receipt slice's content-hash
//!   addressing.
//! - **PredicateType is versioned + Corvid-specific.** URI
//!   `https://corvid-lang.org/attestation/receipt/v1`. Rejects
//!   reusing the SLSA Provenance schema because SLSA Provenance is
//!   about build inputs / outputs, not algebraic-effect deltas —
//!   Corvid receipts need their own predicate type.
//! - **Statement v1 only.** No v0.1 fallback. v1 is the current
//!   stable and everything modern consumes it.
//! - **Unsigned in-toto output is allowed.** `--format=in-toto`
//!   alone emits a raw Statement suitable for piping to external
//!   signing tools (cosign with their own keys, KMS-backed
//!   signers). Forcing `--sign` would exclude users with their
//!   own signing infrastructure.

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::receipt::{Receipt, Verdict};

/// URI identifying the in-toto Statement schema version this
/// renderer emits. Changing this URI is a breaking change for
/// consumers; bump cautiously.
pub(super) const STATEMENT_V1_TYPE: &str = "https://in-toto.io/Statement/v1";

/// URI identifying the Corvid receipt predicate schema. The `/v1`
/// suffix tracks the receipt's own `schema_version`; a breaking
/// change to the receipt shape bumps both.
pub(super) const CORVID_PREDICATE_TYPE: &str =
    "https://corvid-lang.org/attestation/receipt/v1";

/// DSSE payloadType used when an in-toto Statement is signed. The
/// in-toto spec fixes this string; consumers in the ecosystem
/// (cosign, slsa-verifier) expect it verbatim.
pub(super) const IN_TOTO_DSSE_PAYLOAD_TYPE: &str = "application/vnd.in-toto+json";

#[derive(Debug, Clone, Serialize)]
pub(super) struct Statement<'a> {
    #[serde(rename = "_type")]
    pub stmt_type: &'static str,
    pub subject: Vec<Subject>,
    #[serde(rename = "predicateType")]
    pub predicate_type: &'static str,
    pub predicate: &'a Predicate<'a>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct Subject {
    pub name: String,
    pub digest: SubjectDigest,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct SubjectDigest {
    pub sha256: String,
}

/// The Corvid receipt as an in-toto predicate. We wrap the
/// receipt plus the verdict so a consumer has both the structural
/// delta and the policy outcome in one artifact. This mirrors
/// the JSON renderer's envelope shape for consistency.
#[derive(Debug, Clone, Serialize)]
pub(super) struct Predicate<'a> {
    pub verdict: &'a Verdict,
    pub receipt: &'a Receipt,
}

/// Hex-encoded SHA-256 of the head source bytes. This is the
/// subject.digest.sha256 of the in-toto Statement — "the
/// attestation is about this exact source."
pub(super) fn head_source_digest(head_source: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(head_source);
    hex::encode(h.finalize())
}

/// Render the Statement as pretty JSON with a trailing newline,
/// matching every other format's output convention. Bots that
/// want single-line form can parse + re-emit.
pub(super) fn render_in_toto(
    receipt: &Receipt,
    verdict: &Verdict,
    head_source: &[u8],
    source_path: &str,
) -> String {
    let predicate = Predicate { verdict, receipt };
    let statement = Statement {
        stmt_type: STATEMENT_V1_TYPE,
        subject: vec![Subject {
            name: source_path.to_string(),
            digest: SubjectDigest {
                sha256: head_source_digest(head_source),
            },
        }],
        predicate_type: CORVID_PREDICATE_TYPE,
        predicate: &predicate,
    };
    let mut s = serde_json::to_string_pretty(&statement)
        .expect("in-toto Statement is trivially serializable");
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace_diff::impact::TraceImpact;
    use crate::trace_diff::narrative::ReceiptNarrative;
    use crate::trace_diff::receipt::RECEIPT_SCHEMA_VERSION;

    fn fixture_receipt() -> Receipt {
        Receipt {
            schema_version: RECEIPT_SCHEMA_VERSION,
            base_sha: "base_sha_value".into(),
            head_sha: "head_sha_value".into(),
            source_path: "src/agent.cor".into(),
            deltas: vec![],
            impact: TraceImpact::empty(),
            narrative: ReceiptNarrative::empty(),
            narrative_rejected: false,
        }
    }

    fn fixture_verdict() -> Verdict {
        Verdict {
            ok: true,
            flags: vec![],
        }
    }

    #[test]
    fn head_source_digest_is_stable_sha256_hex() {
        let d = head_source_digest(b"hello");
        assert_eq!(
            d,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        assert_eq!(d.len(), 64);
    }

    #[test]
    fn render_in_toto_produces_valid_statement_v1() {
        let r = fixture_receipt();
        let v = fixture_verdict();
        let out = render_in_toto(&r, &v, b"some source bytes", "src/agent.cor");
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();

        assert_eq!(parsed["_type"], STATEMENT_V1_TYPE);
        assert_eq!(parsed["predicateType"], CORVID_PREDICATE_TYPE);
        assert_eq!(parsed["subject"][0]["name"], "src/agent.cor");
        assert_eq!(
            parsed["subject"][0]["digest"]["sha256"],
            head_source_digest(b"some source bytes")
        );
        assert_eq!(parsed["predicate"]["verdict"]["ok"], true);
        assert_eq!(parsed["predicate"]["receipt"]["schema_version"], 2);
    }

    #[test]
    fn render_in_toto_bundles_verdict_and_receipt() {
        // Regression against the shape: `predicate` must contain
        // both `verdict` and `receipt` at the top level, matching
        // the JSON format. Consumers that parse Corvid predicates
        // should see a stable layout across versions.
        let r = fixture_receipt();
        let v = fixture_verdict();
        let out = render_in_toto(&r, &v, b"x", "src/agent.cor");
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();

        assert!(parsed["predicate"]["verdict"].is_object());
        assert!(parsed["predicate"]["receipt"].is_object());
    }

    #[test]
    fn digest_changes_with_content() {
        let a = head_source_digest(b"version A");
        let b = head_source_digest(b"version B");
        assert_ne!(a, b);
    }

    #[test]
    fn subject_name_is_source_path_as_given() {
        let r = fixture_receipt();
        let v = fixture_verdict();
        let out = render_in_toto(&r, &v, b"x", "path/with/slashes/thing.cor");
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["subject"][0]["name"], "path/with/slashes/thing.cor");
    }

    #[test]
    fn output_ends_with_newline() {
        let r = fixture_receipt();
        let v = fixture_verdict();
        let out = render_in_toto(&r, &v, b"x", "a.cor");
        assert!(out.ends_with('\n'));
    }
}
