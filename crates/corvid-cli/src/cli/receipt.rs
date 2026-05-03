use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum ReceiptCommand {
    /// Resolve a cached receipt by its SHA-256 hash (or a
    /// unique prefix of at least 8 characters) and print the
    /// canonical JSON to stdout.
    Show {
        /// Receipt hash (full 64-char SHA-256, or a unique
        /// prefix of at least 8 characters).
        hash: String,
    },
    /// Verify a DSSE envelope against an ed25519 verifying key.
    /// Prints the inner receipt payload on success; exits
    /// non-zero with a typed error on any verification failure.
    Verify {
        /// Envelope location: either a filesystem path to a
        /// `.envelope.json` file OR a hash-prefix matching a
        /// cached `<hash>.envelope.json`.
        envelope: String,
        /// Path to the ed25519 verifying key (64 hex chars or
        /// 32 raw bytes).
        #[arg(long, value_name = "KEY_PATH")]
        key: PathBuf,
    },
    /// Verify the embedded `CORVID_ABI_ATTESTATION` of a Corvid
    /// cdylib against an ed25519 verifying key. Confirms the
    /// signature is valid AND that the recovered descriptor JSON
    /// matches the `CORVID_ABI_DESCRIPTOR` symbol - tampering with
    /// either side is detected. Exits 0 on verified, 2 on absent
    /// (no attestation symbol - host policy decides), 1 on every
    /// other failure (signature mismatch / descriptor drift /
    /// malformed envelope).
    VerifyAbi {
        /// Path to the cdylib `.so` / `.dll` / `.dylib`.
        cdylib: PathBuf,
        /// Path to the ed25519 verifying key (64 hex chars or
        /// 32 raw bytes).
        #[arg(long, value_name = "KEY_PATH")]
        key: PathBuf,
    },
}
