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

pub struct TargetBuildOutput {
    pub source: String,
    pub output_path: Option<PathBuf>,
    pub header_path: Option<PathBuf>,
    pub abi_descriptor_path: Option<PathBuf>,
    pub diagnostics: Vec<Diagnostic>,
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

struct FrontendBundle {
    source: String,
    file: corvid_ast::File,
    resolved: Resolved,
    checked: Checked,
    ir: IrFile,
    effect_registry: EffectRegistry,
}

struct CatalogDescriptorOutput {
    json: String,
    embedded_bytes: Vec<u8>,
}

pub fn build_target_to_disk(
    source_path: &Path,
    target: BuildTarget,
    emit_header: bool,
    emit_abi_descriptor: bool,
    extra_tool_libs: &[&Path],
) -> anyhow::Result<TargetBuildOutput> {
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
                        None,
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

fn emit_catalog_descriptor(
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

fn build_frontend_bundle(
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
