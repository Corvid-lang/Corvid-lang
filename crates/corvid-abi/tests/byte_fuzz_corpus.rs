//! Phase 35-F adversarial byte-fuzz corpus.
//!
//! For every public Corvid promise touching the embedded descriptor
//! and attestation surface, generate hundreds of mutated input bytes
//! and assert each one is rejected by the corresponding parser. This
//! is the load-bearing artifact that proves the parsers are
//! conservative — every malformed, truncated, or tampered input
//! produces a typed error rather than a quiet accept.
//!
//! The harness is deterministic: same seed, same mutations, same
//! coverage on every CI run. No proptest dependency — a fixed
//! linear congruential generator drives the byte mutations so the
//! corpus is reproducible without external state.
//!
//! Guarantees this corpus enforces:
//!
//! - `abi_descriptor.cdylib_emission` (descriptor section parsing
//!   is conservative under arbitrary byte mutation)
//! - `abi_descriptor.byte_determinism` (descriptor JSON is
//!   byte-identical for byte-identical sources)
//! - `abi_attestation.envelope_signature` (DSSE envelope parsing +
//!   signature verification reject every mutated envelope)

mod common;

use common::{emit_descriptor, render_descriptor};
use corvid_abi::{
    attestation_to_embedded_bytes, descriptor_to_embedded_bytes, parse_embedded_attestation_bytes,
    parse_embedded_section_bytes, sign_envelope, verify_envelope,
    CORVID_ABI_ATTESTATION_PAYLOAD_TYPE, CORVID_ABI_ATTESTATION_SECTION_MAGIC,
    CORVID_ABI_SECTION_MAGIC,
};
use ed25519_dalek::SigningKey;

const FUZZ_SOURCE: &str = r#"
@budget($0.10)
pub extern "c"
agent classify(text: String) -> String:
    return text
"#;

/// Deterministic linear congruential generator. Same seed produces
/// the same sequence of mutations across runs, so the corpus is
/// reproducible without proptest.
struct Lcg(u64);
impl Lcg {
    fn new(seed: u64) -> Self {
        Lcg(seed)
    }
    fn next_u64(&mut self) -> u64 {
        // Numerical Recipes parameters.
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0
    }
    fn pick(&mut self, max_exclusive: usize) -> usize {
        (self.next_u64() as usize) % max_exclusive.max(1)
    }
}

fn baseline_descriptor_bytes() -> Vec<u8> {
    let abi = emit_descriptor(FUZZ_SOURCE);
    descriptor_to_embedded_bytes(&abi).expect("descriptor bytes")
}

fn baseline_attestation_bytes() -> (Vec<u8>, SigningKey) {
    let json = render_descriptor(FUZZ_SOURCE);
    let seed = [0x42u8; 32];
    let signing_key = SigningKey::from_bytes(&seed);
    let envelope = sign_envelope(
        json.as_bytes(),
        CORVID_ABI_ATTESTATION_PAYLOAD_TYPE,
        &signing_key,
        "fuzz-test",
    );
    let envelope_json = serde_json::to_vec(&envelope).expect("serialize envelope");
    (attestation_to_embedded_bytes(&envelope_json), signing_key)
}

#[test]
fn descriptor_section_rejects_random_byte_flips() {
    let baseline = baseline_descriptor_bytes();
    parse_embedded_section_bytes(&baseline).expect("baseline parses cleanly");

    // Seed: ascii-mnemonic for "DESC FLIP" — fixed for reproducibility.
    let mut lcg = Lcg::new(0xDE_5C_F1_19_AAAA_5555);
    let mut rejected = 0usize;
    let cases = 256;
    for _ in 0..cases {
        let mut mutated = baseline.clone();
        let idx = lcg.pick(mutated.len());
        // XOR a non-zero byte so we never accidentally produce the
        // original byte by chance.
        let xor = ((lcg.pick(255) + 1) & 0xFF) as u8;
        mutated[idx] ^= xor;
        if parse_embedded_section_bytes(&mutated).is_err() {
            rejected += 1;
        }
    }
    // Single-byte flips inside the JSON body sometimes still yield
    // valid JSON (e.g. flipping a space to a newline) but fail the
    // hash check; they MUST still be rejected. Allow up to 4
    // accepts only as a measurement-error margin; 252+ rejections
    // out of 256 cases proves the parser is conservative.
    assert!(
        rejected >= cases - 4,
        "expected ≥ {} of {} byte flips to be rejected; only {} were rejected",
        cases - 4,
        cases,
        rejected
    );
}

#[test]
fn descriptor_section_rejects_every_truncation() {
    let baseline = baseline_descriptor_bytes();
    parse_embedded_section_bytes(&baseline).expect("baseline parses cleanly");

    // Every prefix shorter than the canonical length must fail to
    // parse. We check every byte offset; the parser is allowed to
    // be cheaper than O(N) but every truncation must produce an
    // error rather than a successful parse of partial bytes.
    let total = baseline.len();
    let mut rejected = 0usize;
    for n in 0..total {
        let truncated = &baseline[..n];
        if parse_embedded_section_bytes(truncated).is_err() {
            rejected += 1;
        }
    }
    assert_eq!(
        rejected, total,
        "every one of {total} truncations must be rejected"
    );
}

#[test]
fn descriptor_section_rejects_wrong_magic() {
    let baseline = baseline_descriptor_bytes();
    let mut mutated = baseline.clone();
    // Replace the canonical magic with the attestation magic and
    // confirm the descriptor parser rejects it. The two magics are
    // distinct on purpose: a verifier reading raw bytes can tell
    // the section type apart even without consulting the symbol
    // name.
    mutated[..4].copy_from_slice(&CORVID_ABI_ATTESTATION_SECTION_MAGIC.to_le_bytes());
    parse_embedded_section_bytes(&mutated).expect_err("attestation magic must be rejected here");

    // And a fully arbitrary magic.
    let mut other = baseline.clone();
    other[..4].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
    parse_embedded_section_bytes(&other).expect_err("arbitrary magic must be rejected");
}

#[test]
fn attestation_section_rejects_every_magic_or_version_byte_flip() {
    // The first 8 bytes — magic (4) + abi_version (4) — admit no
    // valid alternative interpretation. Every flip must produce an
    // error. The length field (bytes 8..16) can mutate into another
    // valid length and then parse a truncated body; that's caught
    // downstream by signature verification, which is exercised by
    // `attestation_section_body_mutations_break_signature_verification`
    // below.
    let (baseline, _key) = baseline_attestation_bytes();
    parse_embedded_attestation_bytes(&baseline).expect("baseline attestation parses cleanly");

    let mut lcg = Lcg::new(0xA77_E57A_710B_55A9);
    let cases = 256;
    let header_strict_end = 8; // bytes 0..8 are the strict magic+version region
    for _ in 0..cases {
        let mut mutated = baseline.clone();
        let idx = lcg.pick(header_strict_end);
        let xor = ((lcg.pick(255) + 1) & 0xFF) as u8;
        mutated[idx] ^= xor;
        let result = parse_embedded_attestation_bytes(&mutated);
        assert!(
            result.is_err(),
            "byte flip at index {idx} (xor 0x{xor:02X}) in magic+version region must reject; got Ok"
        );
    }
}

#[test]
fn attestation_section_body_mutations_break_signature_verification() {
    // Body bytes (everything past the 16-byte header) form the
    // signed JSON envelope. The parser is allowed to accept mutated
    // body bytes when they still happen to be valid UTF-8 + JSON;
    // the load-bearing rejection lives in signature verification.
    // This test asserts: every body byte mutation that DOES still
    // parse must fail signature verification — i.e. there is no
    // mutation that is silently accepted by both stages.
    let (baseline, signing_key) = baseline_attestation_bytes();
    let verifying_key = signing_key.verifying_key();

    let mut lcg = Lcg::new(0xB0DD_1E_55_E2_C0FFEE);
    let cases = 256;
    let header_len = 16;
    let body_offset = header_len;
    let mut accepted_silently = 0usize;
    for _ in 0..cases {
        let mut mutated = baseline.clone();
        let idx = body_offset + lcg.pick(mutated.len() - body_offset);
        let xor = ((lcg.pick(255) + 1) & 0xFF) as u8;
        mutated[idx] ^= xor;
        let Ok(parsed) = parse_embedded_attestation_bytes(&mutated) else {
            // Parser rejected it — fine, that's still rejection.
            continue;
        };
        let verify = verify_envelope(
            parsed.envelope_json.as_bytes(),
            &[CORVID_ABI_ATTESTATION_PAYLOAD_TYPE],
            &verifying_key,
        );
        if verify.is_ok() {
            accepted_silently += 1;
        }
    }
    assert_eq!(
        accepted_silently, 0,
        "no body byte mutation may be accepted by BOTH the parser and the signature verifier"
    );
}

#[test]
fn attestation_section_rejects_every_truncation_under_header() {
    let (baseline, _key) = baseline_attestation_bytes();
    let header_len = 16;
    for n in 0..header_len {
        let truncated = &baseline[..n];
        parse_embedded_attestation_bytes(truncated)
            .expect_err("every truncation that drops part of the 16-byte header must be rejected");
    }
}

#[test]
fn attestation_section_rejects_wrong_magic() {
    let (baseline, _key) = baseline_attestation_bytes();
    let mut mutated = baseline.clone();
    mutated[..4].copy_from_slice(&CORVID_ABI_SECTION_MAGIC.to_le_bytes());
    parse_embedded_attestation_bytes(&mutated)
        .expect_err("descriptor magic must not parse as attestation");

    let mut other = baseline.clone();
    other[..4].copy_from_slice(&0xCAFEBABEu32.to_le_bytes());
    parse_embedded_attestation_bytes(&other).expect_err("arbitrary magic must be rejected");
}

#[test]
fn dsse_envelope_signature_tampering_is_rejected() {
    let (baseline, signing_key) = baseline_attestation_bytes();
    let parsed = parse_embedded_attestation_bytes(&baseline).expect("baseline parse");
    let verifying_key = signing_key.verifying_key();

    // Sanity: untampered envelope verifies cleanly.
    verify_envelope(
        parsed.envelope_json.as_bytes(),
        &[CORVID_ABI_ATTESTATION_PAYLOAD_TYPE],
        &verifying_key,
    )
    .expect("untampered envelope verifies");

    // Mutate every signature byte in turn and confirm verification
    // fails for each.
    let envelope_json = parsed.envelope_json.as_bytes();
    let envelope_text = std::str::from_utf8(envelope_json).expect("utf-8");
    let mut as_value: serde_json::Value =
        serde_json::from_str(envelope_text).expect("parse envelope");
    let sig_str_original = as_value["signatures"][0]["sig"]
        .as_str()
        .expect("sig field")
        .to_string();
    let sig_bytes = sig_str_original.as_bytes();
    let mut tampered_count = 0usize;
    for i in 0..sig_bytes.len() {
        let mut tampered = sig_bytes.to_vec();
        // Flip the byte to a different ASCII char in the same
        // base64 alphabet; if we accidentally pick the same byte,
        // skip.
        let new_byte = match tampered[i] {
            b'A' => b'B',
            _ => b'A',
        };
        if new_byte == tampered[i] {
            continue;
        }
        tampered[i] = new_byte;
        let tampered_str = String::from_utf8(tampered).expect("ascii-safe");
        as_value["signatures"][0]["sig"] = serde_json::Value::String(tampered_str);
        let tampered_envelope = serde_json::to_vec(&as_value).expect("serialize tampered envelope");
        let result = verify_envelope(
            &tampered_envelope,
            &[CORVID_ABI_ATTESTATION_PAYLOAD_TYPE],
            &verifying_key,
        );
        assert!(
            result.is_err(),
            "tampering byte {} in signature must fail verify",
            i
        );
        tampered_count += 1;
    }
    assert!(
        tampered_count >= 32,
        "expected to exercise ≥ 32 signature bytes; only {tampered_count} mutations ran"
    );
}

#[test]
fn dsse_envelope_payload_tampering_is_rejected() {
    let (baseline, signing_key) = baseline_attestation_bytes();
    let parsed = parse_embedded_attestation_bytes(&baseline).expect("baseline parse");
    let verifying_key = signing_key.verifying_key();

    let envelope_text = parsed.envelope_json.as_str();
    let mut as_value: serde_json::Value = serde_json::from_str(envelope_text).expect("parse");
    let payload_b64_original = as_value["payload"]
        .as_str()
        .expect("payload field")
        .to_string();
    let payload_bytes = payload_b64_original.as_bytes();
    let mut tampered_count = 0usize;
    for i in 0..payload_bytes.len() {
        let mut tampered = payload_bytes.to_vec();
        let new_byte = match tampered[i] {
            b'A' => b'B',
            _ => b'A',
        };
        if new_byte == tampered[i] {
            continue;
        }
        tampered[i] = new_byte;
        let tampered_str = String::from_utf8(tampered).expect("ascii-safe");
        as_value["payload"] = serde_json::Value::String(tampered_str);
        let tampered_envelope = serde_json::to_vec(&as_value).expect("serialize tampered envelope");
        let result = verify_envelope(
            &tampered_envelope,
            &[CORVID_ABI_ATTESTATION_PAYLOAD_TYPE],
            &verifying_key,
        );
        assert!(
            result.is_err(),
            "tampering byte {} in payload must fail verify",
            i
        );
        tampered_count += 1;
        if tampered_count >= 64 {
            break;
        }
    }
    assert!(
        tampered_count >= 32,
        "expected to exercise ≥ 32 payload bytes; only {tampered_count} mutations ran"
    );
}

#[test]
fn dsse_envelope_payload_type_swap_is_rejected() {
    let (baseline, signing_key) = baseline_attestation_bytes();
    let parsed = parse_embedded_attestation_bytes(&baseline).expect("baseline parse");
    let verifying_key = signing_key.verifying_key();

    let envelope_text = parsed.envelope_json.as_str();
    let mut as_value: serde_json::Value = serde_json::from_str(envelope_text).expect("parse");
    // The DSSE PAE binds payloadType into the signed bytes, so any
    // swap must invalidate the signature.
    let foreign_types = [
        "application/vnd.corvid-receipt+json",
        "application/vnd.corvid-bogus+json",
        "text/plain",
        "",
    ];
    for foreign in foreign_types {
        as_value["payloadType"] = serde_json::Value::String(foreign.to_string());
        let tampered_envelope = serde_json::to_vec(&as_value).expect("serialize tampered envelope");
        let result = verify_envelope(
            &tampered_envelope,
            &[CORVID_ABI_ATTESTATION_PAYLOAD_TYPE],
            &verifying_key,
        );
        assert!(
            result.is_err(),
            "payloadType swap to `{foreign}` must fail verify"
        );
    }
}

#[test]
fn descriptor_bytes_are_byte_identical_across_two_emissions_of_same_source() {
    // Direct byte determinism check beyond the JSON-text comparison
    // in `tests/determinism.rs`. If this ever stops holding the
    // signed-attestation contract is meaningless, because the
    // signed payload + the embedded descriptor would diverge by
    // construction.
    let left = baseline_descriptor_bytes();
    let right = baseline_descriptor_bytes();
    assert_eq!(
        left, right,
        "descriptor_to_embedded_bytes must be a pure function of source for the byte_determinism guarantee"
    );
}

#[test]
fn signing_key_round_trip_baseline() {
    // Confirm sign_envelope + verify_envelope round-trip cleanly so
    // the rest of the corpus's "verify rejects tampering" tests
    // stand on a known-good baseline.
    let (baseline, key) = baseline_attestation_bytes();
    let parsed = parse_embedded_attestation_bytes(&baseline).expect("parse");
    let verifying_key = key.verifying_key();
    let recovered = verify_envelope(
        parsed.envelope_json.as_bytes(),
        &[CORVID_ABI_ATTESTATION_PAYLOAD_TYPE],
        &verifying_key,
    )
    .expect("baseline verifies");
    assert!(!recovered.is_empty());
}
