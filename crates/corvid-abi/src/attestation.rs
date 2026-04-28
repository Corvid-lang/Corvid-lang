//! Embedded ABI attestation section.
//!
//! A signed Corvid cdylib carries a second exported symbol —
//! [`CORVID_ABI_ATTESTATION_SYMBOL`] — alongside the existing
//! [`crate::CORVID_ABI_DESCRIPTOR_SYMBOL`]. The attestation section
//! wraps a DSSE envelope (Phase 21-inv-H-5-signed format) whose
//! payload is the same descriptor JSON the descriptor section
//! holds. Hosts that load the cdylib can verify the signature
//! against a user-supplied public key and confirm the loaded
//! descriptor matches the one that was signed at build time.
//!
//! Section layout (mirrors [`crate::embedded`]):
//!
//! ```text
//! [magic:u32 LE = 0x41545354]   "ATST"
//! [version:u32 LE]              CORVID_ABI_VERSION
//! [envelope_len:u64 LE]         length of the envelope JSON
//! [envelope_json:UTF-8]         DSSE envelope bytes
//! ```
//!
//! No trailing hash. The DSSE envelope itself binds the signature
//! to the payload via PAE, and the verifier compares the envelope's
//! decoded payload against the loaded descriptor — tampering with
//! either side is detected.
//!
//! The section is intentionally OPT-IN: a cdylib without an
//! attestation symbol still loads and runs. Hosts that require
//! attestation enforce the policy themselves; the runtime verifier
//! reports `Absent` rather than failing closed, mirroring the
//! Phase 21 receipt verification convention.

use crate::CORVID_ABI_VERSION;
use std::fmt;

/// Exported symbol name. Codegen-cl declares this as
/// `Linkage::Export` data when the build is signed.
pub const CORVID_ABI_ATTESTATION_SYMBOL: &str = "CORVID_ABI_ATTESTATION";

/// Section magic. Distinct from [`crate::CORVID_ABI_SECTION_MAGIC`]
/// so a verifier reading raw bytes can tell the two sections apart
/// without consulting the symbol name. ASCII "ATST" little-endian.
pub const CORVID_ABI_ATTESTATION_SECTION_MAGIC: u32 = 0x54535441;

/// DSSE `payloadType` URI for ABI attestations. Distinct from the
/// receipt payload type so a receipt envelope cannot be replayed
/// as an attestation (DSSE PAE binds the type to the signature, so
/// crafted envelope-substitution attacks fail at signature verify
/// time even before any payload-type check).
pub const CORVID_ABI_ATTESTATION_PAYLOAD_TYPE: &str = "application/vnd.corvid-abi-attestation+json";

#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddedAttestationSection {
    pub magic: u32,
    pub abi_version: u32,
    pub envelope_len: u64,
    pub envelope_json: String,
}

#[derive(Debug)]
pub enum EmbeddedAttestationError {
    TooShort { len: usize },
    BadMagic { found: u32 },
    Utf8(std::string::FromUtf8Error),
    LengthOverflow(u64),
    VersionMismatch { found: u32, expected: u32 },
    SymbolLoad(String),
}

impl fmt::Display for EmbeddedAttestationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort { len } => write!(f, "attestation section too short: {len} bytes"),
            Self::BadMagic { found } => write!(f, "bad attestation magic: 0x{found:08X}"),
            Self::Utf8(err) => write!(f, "attestation envelope was not valid UTF-8: {err}"),
            Self::LengthOverflow(len) => write!(f, "attestation envelope length {len} does not fit in memory"),
            Self::VersionMismatch { found, expected } => write!(
                f,
                "attestation abi_version {found} did not match current CORVID_ABI_VERSION {expected}"
            ),
            Self::SymbolLoad(err) => write!(f, "failed to load embedded attestation symbol: {err}"),
        }
    }
}

impl std::error::Error for EmbeddedAttestationError {}

/// Wrap a DSSE envelope's JSON bytes in the embedded attestation
/// section format. Returned bytes go into the
/// [`CORVID_ABI_ATTESTATION_SYMBOL`] data symbol of the cdylib.
pub fn attestation_to_embedded_bytes(envelope_json: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(16 + envelope_json.len());
    bytes.extend_from_slice(&CORVID_ABI_ATTESTATION_SECTION_MAGIC.to_le_bytes());
    bytes.extend_from_slice(&CORVID_ABI_VERSION.to_le_bytes());
    bytes.extend_from_slice(&(envelope_json.len() as u64).to_le_bytes());
    bytes.extend_from_slice(envelope_json);
    bytes
}

/// Parse the bytes from a CORVID_ABI_ATTESTATION symbol.
pub fn parse_embedded_attestation_bytes(
    bytes: &[u8],
) -> Result<EmbeddedAttestationSection, EmbeddedAttestationError> {
    if bytes.len() < 16 {
        return Err(EmbeddedAttestationError::TooShort { len: bytes.len() });
    }
    let magic = u32::from_le_bytes(bytes[0..4].try_into().expect("magic width"));
    if magic != CORVID_ABI_ATTESTATION_SECTION_MAGIC {
        return Err(EmbeddedAttestationError::BadMagic { found: magic });
    }
    let abi_version = u32::from_le_bytes(bytes[4..8].try_into().expect("version width"));
    if abi_version != CORVID_ABI_VERSION {
        return Err(EmbeddedAttestationError::VersionMismatch {
            found: abi_version,
            expected: CORVID_ABI_VERSION,
        });
    }
    let envelope_len = u64::from_le_bytes(bytes[8..16].try_into().expect("len width"));
    let envelope_len_usize = usize::try_from(envelope_len)
        .map_err(|_| EmbeddedAttestationError::LengthOverflow(envelope_len))?;
    let expected_total = 16usize
        .checked_add(envelope_len_usize)
        .ok_or(EmbeddedAttestationError::LengthOverflow(envelope_len))?;
    if bytes.len() < expected_total {
        return Err(EmbeddedAttestationError::TooShort { len: bytes.len() });
    }
    let envelope_bytes = &bytes[16..16 + envelope_len_usize];
    let envelope_json =
        String::from_utf8(envelope_bytes.to_vec()).map_err(EmbeddedAttestationError::Utf8)?;
    Ok(EmbeddedAttestationSection {
        magic,
        abi_version,
        envelope_len,
        envelope_json,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_envelope_bytes() {
        let envelope = br#"{"payloadType":"application/vnd.corvid-abi-attestation+json","payload":"...","signatures":[]}"#;
        let bytes = attestation_to_embedded_bytes(envelope);
        let parsed = parse_embedded_attestation_bytes(&bytes).expect("parse");
        assert_eq!(parsed.magic, CORVID_ABI_ATTESTATION_SECTION_MAGIC);
        assert_eq!(parsed.abi_version, CORVID_ABI_VERSION);
        assert_eq!(parsed.envelope_len as usize, envelope.len());
        assert_eq!(parsed.envelope_json.as_bytes(), envelope);
    }

    #[test]
    fn rejects_descriptor_magic() {
        // Verifier asked to interpret descriptor bytes as an
        // attestation must reject — the two sections are
        // intentionally distinguishable at the byte level.
        let mut bytes = Vec::with_capacity(16);
        bytes.extend_from_slice(&crate::CORVID_ABI_SECTION_MAGIC.to_le_bytes());
        bytes.extend_from_slice(&CORVID_ABI_VERSION.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        let err = parse_embedded_attestation_bytes(&bytes).unwrap_err();
        assert!(matches!(err, EmbeddedAttestationError::BadMagic { .. }));
    }

    #[test]
    fn rejects_truncated_section() {
        let bytes = vec![0u8; 8];
        let err = parse_embedded_attestation_bytes(&bytes).unwrap_err();
        assert!(matches!(err, EmbeddedAttestationError::TooShort { .. }));
    }

    #[test]
    fn rejects_truncated_envelope_payload() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&CORVID_ABI_ATTESTATION_SECTION_MAGIC.to_le_bytes());
        bytes.extend_from_slice(&CORVID_ABI_VERSION.to_le_bytes());
        // Claim 100 bytes of envelope but only provide 16+0.
        bytes.extend_from_slice(&100u64.to_le_bytes());
        let err = parse_embedded_attestation_bytes(&bytes).unwrap_err();
        assert!(matches!(err, EmbeddedAttestationError::TooShort { .. }));
    }
}
