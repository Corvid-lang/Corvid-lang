//! Content-hash verification for local Corvid imports.
//!
//! The parser owns the surface syntax. The module loader owns the
//! trust-boundary check: if a source file is imported with
//! `hash:sha256:<digest>`, these helpers compute the digest over the
//! exact bytes read from disk before the file is parsed or exposed.

use sha2::{Digest, Sha256};

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    encode_hex(&hasher.finalize())
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_hex_matches_known_digest() {
        assert_eq!(
            sha256_hex(b"corvid\n"),
            "d51d10c7a9124875f4d37cfaec7329d24180725e1164cbc3759ce3bdbbaea1dd"
        );
    }
}
