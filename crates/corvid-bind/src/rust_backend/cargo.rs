//! Cargo.toml + README template emission for the Rust binding
//! backend — slice 23 / per-cdylib bindings, decomposed in
//! Phase 20j-A12.
//!
//! Both templates are static-shape per package: name, version,
//! source stem, descriptor hash, bind version, generated_at
//! get interpolated. Nothing else changes between callers.

use crate::BindingContext;

pub(super) fn render_cargo_toml(context: &BindingContext) -> String {
    format!(
        r#"[package]
name = "{name}"
version = "0.0.1"
edition = "2021"
license = "MIT OR Apache-2.0"
description = "Generated Corvid Rust bindings for {source}"

[dependencies]
libloading = "0.8"
serde = {{ version = "1", features = ["derive"] }}
serde_json = "1"

[workspace]
"#,
        name = context.package_name,
        source = context.source_stem,
    )
}

pub(super) fn render_readme(context: &BindingContext) -> String {
    format!(
        "# Generated Corvid Rust bindings\n\nGenerated from `{source}`.\nDescriptor sha256: `{hash}`.\ncorvid-bind version: `{version}`.\nDescriptor generated_at: `{generated_at}`.\n",
        source = context.abi.source_path,
        hash = context.descriptor_hash_hex,
        version = context.bind_version,
        generated_at = context.abi.generated_at
    )
}
