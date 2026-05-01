//! Catalog-ABI descriptor emission.
//!
//! `corvid build --target catalog` writes the ABI descriptor JSON
//! to disk; the cdylib build path embeds the same descriptor as a
//! signed payload. Both flow through the helpers here:
//! `build_catalog_descriptor_for_source` is the public entry that
//! runs the frontend and emits the JSON, and `emit_catalog_descriptor`
//! is the private worker that calls into `corvid_abi` to actually
//! produce the typed descriptor + the byte-stable JSON / embedded
//! payload.

use std::path::Path;

use crate::load_corvid_config_for;

use super::{build_frontend_bundle, AbiBuildOutput, FrontendBundle};

pub(super) struct CatalogDescriptorOutput {
    pub json: String,
    pub embedded_bytes: Vec<u8>,
}

pub fn build_catalog_descriptor_for_source(source_path: &Path) -> anyhow::Result<AbiBuildOutput> {
    let source = std::fs::read_to_string(source_path)
        .map_err(|e| anyhow::anyhow!("cannot read `{}`: {}", source_path.display(), e))?;
    let config = load_corvid_config_for(source_path);
    match build_frontend_bundle(&source, source_path, config.as_ref()) {
        Err(diagnostics) => Ok(AbiBuildOutput {
            source,
            descriptor_json: None,
            descriptor_hash: None,
            diagnostics,
        }),
        Ok(frontend) => {
            let descriptor = emit_catalog_descriptor(source_path, &frontend)?;
            let hash = corvid_abi::hash_json_str(&descriptor.json);
            Ok(AbiBuildOutput {
                source,
                descriptor_json: Some(descriptor.json),
                descriptor_hash: Some(hash),
                diagnostics: Vec::new(),
            })
        }
    }
}

pub(super) fn emit_catalog_descriptor(
    source_path: &Path,
    frontend: &FrontendBundle,
) -> anyhow::Result<CatalogDescriptorOutput> {
    // Phase 22-C embeds and hashes the descriptor inside the produced cdylib,
    // so the JSON body must be byte-stable across identical builds.
    let generated_at = "1970-01-01T00:00:00Z".to_string();
    let normalized_source_path = corvid_abi::normalize_source_path(&source_path.to_string_lossy());
    let descriptor = corvid_abi::emit_catalog_abi(
        &frontend.file,
        &frontend.resolved,
        &frontend.checked,
        &frontend.ir,
        &frontend.effect_registry,
        &corvid_abi::EmitOptions {
            source_path: &normalized_source_path,
            source_text: &frontend.source,
            compiler_version: env!("CARGO_PKG_VERSION"),
            generated_at: &generated_at,
        },
    );
    let json = corvid_abi::render_descriptor_json(&descriptor)
        .map_err(|e| anyhow::anyhow!("serialize descriptor: {e}"))?;
    let embedded_bytes = corvid_abi::descriptor_to_embedded_bytes(&descriptor)
        .map_err(|e| anyhow::anyhow!("encode embedded descriptor: {e}"))?;
    Ok(CatalogDescriptorOutput {
        json,
        embedded_bytes,
    })
}
