//! Build-to-disk helpers — compile a Corvid source file and write
//! the emitted artifact (Python or native binary) to `target/`.
//!
//! `corvid build <file>` and `corvid build --target native <file>`
//! both route through here.
//!
//! Extracted from `lib.rs` as part of Phase 20i responsibility
//! decomposition (20i-audit-driver-d).

use super::{
    compile_to_ir_with_config_at_path, compile_with_config_at_path, load_corvid_config_for,
    lower_driver_file, typecheck_driver_file, Diagnostic,
};
pub use corvid_codegen_cl::BuildTarget;
use corvid_ir::IrFile;
use corvid_resolve::{resolve, Resolved};
use corvid_syntax::{lex, parse_file};
use corvid_types::{Checked, CorvidConfig, EffectRegistry};
use std::path::{Path, PathBuf};

mod catalog_descriptor;
mod claim_coverage;
mod server_render;
pub use catalog_descriptor::build_catalog_descriptor_for_source;
use catalog_descriptor::emit_catalog_descriptor;
use claim_coverage::validate_signed_claim_coverage;
use server_render::{
    render_axum_server_source, render_server_cargo_toml, server_binary_name_for_package,
    server_binary_path_for, server_package_name,
};

#[cfg(test)]
mod tests;

/// Compile `source_path` and write the generated Python to disk.
///
/// Layout convention:
///   * If the source is inside a `src/` directory, output goes to a sibling
///     `target/py/<stem>.py` relative to that `src/`.
///   * Otherwise, output goes alongside the source in `./target/py/<stem>.py`.
pub fn build_to_disk(source_path: &Path) -> anyhow::Result<BuildOutput> {
    let source = std::fs::read_to_string(source_path)
        .map_err(|e| anyhow::anyhow!("cannot read `{}`: {}", source_path.display(), e))?;

    let config = load_corvid_config_for(source_path);
    let result = compile_with_config_at_path(&source, source_path, config.as_ref());

    if !result.ok() {
        return Ok(BuildOutput {
            source,
            output_path: None,
            diagnostics: result.diagnostics,
        });
    }

    let out_path = output_path_for(source_path);
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let py = result.python_source.expect("codegen produced no source");
    std::fs::write(&out_path, &py)?;

    Ok(BuildOutput {
        source,
        output_path: Some(out_path),
        diagnostics: Vec::new(),
    })
}

pub struct BuildOutput {
    pub source: String,
    pub output_path: Option<PathBuf>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Compile `source_path` to a native binary under `<project>/target/bin/`.
///
/// Layout convention mirrors `build_to_disk`: if the source is inside a
/// `src/` directory, output goes to a sibling `target/bin/<stem>[.exe]`.
/// Otherwise, output goes alongside the source in `./target/bin/`.
pub fn build_native_to_disk(
    source_path: &Path,
    extra_tool_libs: &[&Path],
) -> anyhow::Result<NativeBuildOutput> {
    let source = std::fs::read_to_string(source_path)
        .map_err(|e| anyhow::anyhow!("cannot read `{}`: {}", source_path.display(), e))?;

    let config = load_corvid_config_for(source_path);
    match compile_to_ir_with_config_at_path(&source, source_path, config.as_ref()) {
        Err(diagnostics) => Ok(NativeBuildOutput {
            source,
            output_path: None,
            diagnostics,
        }),
        Ok(ir) => {
            let bin_dir = native_output_dir_for(source_path);
            let stem = source_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("program")
                .to_string();
            let requested = bin_dir.join(&stem);
            // Production users pass `--with-tools-lib` to the CLI;
            // this path is the one hit by that flow and by tool-free
            // `corvid build --target=native`.
            // Empty tools-lib list = no user tool crates linked — tool-using
            // programs fail at link time with an unresolved-symbol
            // error that surfaces the missing tool by name.
            let produced =
                corvid_codegen_cl::build_native_to_disk(&ir, &stem, &requested, extra_tool_libs)
                    .map_err(|e| anyhow::anyhow!("native codegen failed: {e}"))?;
            Ok(NativeBuildOutput {
                source,
                output_path: Some(produced),
                diagnostics: Vec::new(),
            })
        }
    }
}

pub struct NativeBuildOutput {
    pub source: String,
    pub output_path: Option<PathBuf>,
    pub diagnostics: Vec<Diagnostic>,
}

pub struct ServerBuildOutput {
    pub source: String,
    pub output_path: Option<PathBuf>,
    pub handler_path: Option<PathBuf>,
    pub diagnostics: Vec<Diagnostic>,
}

pub struct TargetBuildOutput {
    pub source: String,
    pub output_path: Option<PathBuf>,
    pub header_path: Option<PathBuf>,
    pub abi_descriptor_path: Option<PathBuf>,
    pub diagnostics: Vec<Diagnostic>,
    /// True when an ed25519 attestation envelope was signed and
    /// embedded into the cdylib at this build. False for unsigned
    /// builds, every non-cdylib target, and any frontend-error path.
    pub signed: bool,
}

/// Caller-provided signing material for the cdylib path. CLI parses
/// the key + label once at flag-parse time and hands the resolved
/// pair to the driver; the driver does not re-touch env vars or key
/// files itself.
pub struct SigningRequest {
    pub key: ed25519_dalek::SigningKey,
    pub key_id: String,
}

pub struct WasmBuildOutput {
    pub source: String,
    pub wasm_path: Option<PathBuf>,
    pub js_loader_path: Option<PathBuf>,
    pub ts_types_path: Option<PathBuf>,
    pub manifest_path: Option<PathBuf>,
    pub diagnostics: Vec<Diagnostic>,
}

pub struct AbiBuildOutput {
    pub source: String,
    pub descriptor_json: Option<String>,
    pub descriptor_hash: Option<[u8; 32]>,
    pub diagnostics: Vec<Diagnostic>,
}

pub(super) struct FrontendBundle {
    pub source: String,
    pub file: corvid_ast::File,
    pub resolved: Resolved,
    pub checked: Checked,
    pub ir: IrFile,
    pub effect_registry: EffectRegistry,
}



pub fn build_target_to_disk(
    source_path: &Path,
    target: BuildTarget,
    emit_header: bool,
    emit_abi_descriptor: bool,
    extra_tool_libs: &[&Path],
    signing: Option<SigningRequest>,
) -> anyhow::Result<TargetBuildOutput> {
    if signing.is_some() && !matches!(target, BuildTarget::Cdylib) {
        return Err(anyhow::anyhow!(
            "signing is only supported for cdylib targets — descriptor attestations are bound to the embedded cdylib descriptor symbol"
        ));
    }
    let source = std::fs::read_to_string(source_path)
        .map_err(|e| anyhow::anyhow!("cannot read `{}`: {}", source_path.display(), e))?;

    let config = load_corvid_config_for(source_path);
    match build_frontend_bundle(&source, source_path, config.as_ref()) {
        Err(diagnostics) => Ok(TargetBuildOutput {
            source,
            output_path: None,
            header_path: None,
            abi_descriptor_path: None,
            diagnostics,
            signed: false,
        }),
        Ok(frontend) => {
            let out_dir = target_output_dir_for(source_path, target);
            let stem = source_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("program")
                .to_string();
            let requested = out_dir.join(&stem);
            let catalog_descriptor = if matches!(target, BuildTarget::Cdylib) {
                Some(emit_catalog_descriptor(source_path, &frontend)?)
            } else {
                None
            };
            if signing.is_some() {
                let descriptor = catalog_descriptor
                    .as_ref()
                    .expect("signed builds are only supported for cdylib descriptors");
                validate_signed_claim_coverage(&frontend.file, &descriptor.json)?;
            }
            // Sign the descriptor JSON now so the envelope is locked
            // before any codegen happens. The DSSE PAE binds the
            // signature to (payloadType, payload), so even if the
            // verifier later sees a binary with a tampered descriptor
            // section, the signature won't match the recovered
            // payload.
            let attestation_bytes = match (&catalog_descriptor, &signing) {
                (Some(descriptor), Some(req)) => {
                    let envelope = corvid_abi::sign_envelope(
                        descriptor.json.as_bytes(),
                        corvid_abi::CORVID_ABI_ATTESTATION_PAYLOAD_TYPE,
                        &req.key,
                        &req.key_id,
                    );
                    let envelope_json = serde_json::to_vec(&envelope)
                        .map_err(|e| anyhow::anyhow!("serialize attestation envelope: {e}"))?;
                    Some(corvid_abi::attestation_to_embedded_bytes(&envelope_json))
                }
                _ => None,
            };
            let signed = attestation_bytes.is_some();
            let produced = match target {
                BuildTarget::Native => corvid_codegen_cl::build_native_to_disk(
                    &frontend.ir,
                    &stem,
                    &requested,
                    extra_tool_libs,
                ),
                BuildTarget::Cdylib | BuildTarget::Staticlib => {
                    corvid_codegen_cl::build_library_to_disk(
                        &frontend.ir,
                        &stem,
                        &requested,
                        target,
                        extra_tool_libs,
                        catalog_descriptor
                            .as_ref()
                            .map(|descriptor| descriptor.embedded_bytes.as_slice()),
                        attestation_bytes.as_deref(),
                    )
                }
            }
            .map_err(|e| anyhow::anyhow!("native codegen failed: {e}"))?;

            let header_path = if emit_header {
                let header = corvid_c_header::emit_header(
                    &frontend.ir,
                    &corvid_c_header::HeaderOptions {
                        library_name: stem.clone(),
                    },
                );
                let path = out_dir.join(format!("lib_{stem}.h"));
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, header)?;
                Some(path)
            } else {
                None
            };

            let abi_descriptor_path = if emit_abi_descriptor {
                let descriptor_json = if let Some(descriptor) = &catalog_descriptor {
                    descriptor.json.clone()
                } else {
                    emit_catalog_descriptor(source_path, &frontend)?.json
                };
                let path = out_dir.join(format!("{stem}.corvid-abi.json"));
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, descriptor_json)?;
                Some(path)
            } else {
                None
            };

            Ok(TargetBuildOutput {
                source,
                output_path: Some(produced),
                header_path,
                abi_descriptor_path,
                diagnostics: Vec::new(),
                signed,
            })
        }
    }
}

pub fn build_wasm_to_disk(source_path: &Path) -> anyhow::Result<WasmBuildOutput> {
    let source = std::fs::read_to_string(source_path)
        .map_err(|e| anyhow::anyhow!("cannot read `{}`: {}", source_path.display(), e))?;

    let config = load_corvid_config_for(source_path);
    match build_frontend_bundle(&source, source_path, config.as_ref()) {
        Err(diagnostics) => Ok(WasmBuildOutput {
            source,
            wasm_path: None,
            js_loader_path: None,
            ts_types_path: None,
            manifest_path: None,
            diagnostics,
        }),
        Ok(frontend) => {
            let out_dir = wasm_output_dir_for(source_path);
            let stem = source_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("program")
                .to_string();
            let artifacts = corvid_codegen_wasm::emit_wasm_artifacts(&frontend.ir, &stem)
                .map_err(|e| anyhow::anyhow!("wasm codegen failed: {e}"))?;
            std::fs::create_dir_all(&out_dir)?;
            let wasm_path = out_dir.join(format!("{stem}.wasm"));
            let js_loader_path = out_dir.join(format!("{stem}.js"));
            let ts_types_path = out_dir.join(format!("{stem}.d.ts"));
            let manifest_path = out_dir.join(format!("{stem}.corvid-wasm.json"));
            std::fs::write(&wasm_path, artifacts.wasm)?;
            std::fs::write(&js_loader_path, artifacts.js_loader)?;
            std::fs::write(&ts_types_path, artifacts.ts_types)?;
            std::fs::write(&manifest_path, artifacts.manifest_json)?;
            Ok(WasmBuildOutput {
                source,
                wasm_path: Some(wasm_path),
                js_loader_path: Some(js_loader_path),
                ts_types_path: Some(ts_types_path),
                manifest_path: Some(manifest_path),
                diagnostics: Vec::new(),
            })
        }
    }
}

pub fn build_server_to_disk(
    source_path: &Path,
    extra_tool_libs: &[&Path],
) -> anyhow::Result<ServerBuildOutput> {
    let native = build_native_to_disk(source_path, extra_tool_libs)?;
    let Some(handler_path) = native.output_path else {
        return Ok(ServerBuildOutput {
            source: native.source,
            output_path: None,
            handler_path: None,
            diagnostics: native.diagnostics,
        });
    };

    let server_dir = server_output_dir_for(source_path);
    std::fs::create_dir_all(&server_dir)?;
    let stem = source_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("program");
    let source_rs = server_dir.join("src").join("main.rs");
    let output_path = server_binary_path_for(&server_dir, stem);
    std::fs::create_dir_all(source_rs.parent().expect("server source dir"))?;
    std::fs::write(&source_rs, render_axum_server_source(&handler_path))?;
    std::fs::write(
        server_dir.join("Cargo.toml"),
        render_server_cargo_toml(&server_package_name(stem)),
    )?;

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = std::process::Command::new(cargo)
        .arg("build")
        .arg("--release")
        .arg("--manifest-path")
        .arg(server_dir.join("Cargo.toml"))
        .status()
        .map_err(|err| anyhow::anyhow!("failed to invoke cargo for server wrapper: {err}"))?;
    if !status.success() {
        return Err(anyhow::anyhow!(
            "server wrapper compilation failed with status {status}"
        ));
    }
    let built = server_dir
        .join("target")
        .join("release")
        .join(server_binary_name_for_package(&server_package_name(stem)));
    std::fs::copy(&built, &output_path).map_err(|err| {
        anyhow::anyhow!(
            "failed to copy server wrapper `{}` to `{}`: {err}",
            built.display(),
            output_path.display()
        )
    })?;

    Ok(ServerBuildOutput {
        source: native.source,
        output_path: Some(output_path),
        handler_path: Some(handler_path),
        diagnostics: Vec::new(),
    })
}


pub(super) fn build_frontend_bundle(
    source: &str,
    source_path: &Path,
    config: Option<&CorvidConfig>,
) -> Result<FrontendBundle, Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();
    let tokens = match lex(source) {
        Ok(tokens) => tokens,
        Err(errs) => {
            diagnostics.extend(errs.into_iter().map(Diagnostic::from));
            return Err(diagnostics);
        }
    };
    let (file, parse_errs) = parse_file(&tokens);
    diagnostics.extend(parse_errs.into_iter().map(Diagnostic::from));
    let resolved = resolve(&file);
    diagnostics.extend(resolved.errors.iter().cloned().map(Diagnostic::from));
    let typechecked = typecheck_driver_file(&file, &resolved, source_path, config);
    diagnostics.extend(typechecked.diagnostics);
    diagnostics.extend(
        typechecked
            .result
            .checked
            .errors
            .iter()
            .cloned()
            .map(Diagnostic::from),
    );
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    let effect_decls = file
        .decls
        .iter()
        .filter_map(|decl| match decl {
            corvid_ast::Decl::Effect(effect) => Some(effect.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let effect_registry = EffectRegistry::from_decls_with_config(&effect_decls, config);
    let checked = typechecked.result.checked.clone();
    let ir = lower_driver_file(&file, &resolved, &typechecked.result);
    Ok(FrontendBundle {
        source: source.to_string(),
        file,
        resolved,
        checked,
        ir,
        effect_registry,
    })
}

pub(super) fn native_output_dir_for(source_path: &Path) -> PathBuf {
    let mut ancestor: Option<&Path> = source_path.parent();
    while let Some(dir) = ancestor {
        if dir.file_name().map(|n| n == "src").unwrap_or(false) {
            if let Some(project_root) = dir.parent() {
                return project_root.join("target").join("bin");
            }
        }
        ancestor = dir.parent();
    }
    let parent = source_path.parent().unwrap_or_else(|| Path::new("."));
    parent.join("target").join("bin")
}

pub(super) fn target_output_dir_for(source_path: &Path, target: BuildTarget) -> PathBuf {
    match target {
        BuildTarget::Native => native_output_dir_for(source_path),
        BuildTarget::Cdylib | BuildTarget::Staticlib => {
            let mut ancestor: Option<&Path> = source_path.parent();
            while let Some(dir) = ancestor {
                if dir.file_name().map(|n| n == "src").unwrap_or(false) {
                    if let Some(project_root) = dir.parent() {
                        return project_root.join("target").join("release");
                    }
                }
                ancestor = dir.parent();
            }
            let parent = source_path.parent().unwrap_or_else(|| Path::new("."));
            parent.join("target").join("release")
        }
    }
}

pub(super) fn server_output_dir_for(source_path: &Path) -> PathBuf {
    let mut ancestor: Option<&Path> = source_path.parent();
    while let Some(dir) = ancestor {
        if dir.file_name().map(|n| n == "src").unwrap_or(false) {
            if let Some(project_root) = dir.parent() {
                return project_root.join("target").join("server");
            }
        }
        ancestor = dir.parent();
    }
    let parent = source_path.parent().unwrap_or_else(|| Path::new("."));
    parent.join("target").join("server")
}


pub(super) fn wasm_output_dir_for(source_path: &Path) -> PathBuf {
    let mut ancestor: Option<&Path> = source_path.parent();
    while let Some(dir) = ancestor {
        if dir.file_name().map(|n| n == "src").unwrap_or(false) {
            if let Some(project_root) = dir.parent() {
                return project_root.join("target").join("wasm");
            }
        }
        ancestor = dir.parent();
    }
    let parent = source_path.parent().unwrap_or_else(|| Path::new("."));
    parent.join("target").join("wasm")
}

pub(super) fn output_path_for(source_path: &Path) -> PathBuf {
    let stem = source_path.file_stem().unwrap_or_default();
    let py_name = format!("{}.py", stem.to_string_lossy());

    // Find the nearest enclosing `src` directory by walking up.
    let mut ancestor: Option<&Path> = source_path.parent();
    while let Some(dir) = ancestor {
        if dir.file_name().map(|n| n == "src").unwrap_or(false) {
            if let Some(project_root) = dir.parent() {
                return project_root.join("target").join("py").join(py_name);
            }
        }
        ancestor = dir.parent();
    }

    // Default: alongside the source, in a `target/py/` subdir.
    let parent = source_path.parent().unwrap_or_else(|| Path::new("."));
    parent.join("target").join("py").join(py_name)
}

