//! `corvid package` clap argument tree — slice 33 / package
//! manager surface, decomposed in Phase 20j-A1.
//!
//! Owns the [`PackageCommand`] subcommand enum that the
//! `corvid package metadata|verify-registry|verify-lock|publish`
//! dispatch arms consume.

use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum PackageCommand {
    /// Render the public semantic metadata page for a source package.
    Metadata {
        /// Source `.cor` file to inspect.
        source: PathBuf,
        /// Scoped package name, e.g. `@scope/name`.
        #[arg(long)]
        name: String,
        /// Semantic version to display in install snippets.
        #[arg(long)]
        version: String,
        /// Optional package signature to render on the metadata page.
        #[arg(long)]
        signature: Option<String>,
        /// Emit structured JSON instead of Markdown.
        #[arg(long)]
        json: bool,
    },
    /// Verify a registry index and all referenced source artifacts.
    VerifyRegistry {
        /// Registry index URL, local index.toml, or registry directory.
        registry: String,
        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Verify corvid.toml and Corvid.lock agree with package policy.
    VerifyLock {
        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Publish a signed source package into a registry directory.
    Publish {
        /// Source `.cor` file to publish.
        source: PathBuf,
        /// Scoped package name, e.g. `@scope/name`.
        #[arg(long)]
        name: String,
        /// Semantic version, e.g. `1.2.3`.
        #[arg(long)]
        version: String,
        /// Registry output directory. `index.toml` is created/updated here.
        #[arg(long, value_name = "DIR")]
        out: PathBuf,
        /// Public URL prefix where copied package artifacts will be served.
        #[arg(long, value_name = "URL")]
        url_base: String,
        /// 32-byte Ed25519 signing seed as 64 hex chars.
        #[arg(long, value_name = "HEX")]
        key: String,
        /// Key identifier embedded in the package signature.
        #[arg(long, default_value = "corvid-package")]
        key_id: String,
    },
}
