//! Build-to-disk helpers — compile a Corvid source file and write
//! the emitted artifact (Python or native binary) to `target/`.
//!
//! `corvid build <file>` and `corvid build --target native <file>`
//! both route through here.
//!
//! Extracted from `lib.rs` as part of Phase 20i responsibility
//! decomposition (20i-audit-driver-d).

use super::{compile_to_ir_with_config, compile_with_config, load_corvid_config_for, Diagnostic};
use corvid_codegen_py::emit;
pub use corvid_codegen_cl::BuildTarget;
use corvid_ir::{lower, IrFile};
use corvid_resolve::{resolve, Resolved};
use corvid_syntax::{lex, parse_file};
use corvid_types::{typecheck_with_config, Checked, CorvidConfig, EffectRegistry};
use std::path::{Path, PathBuf};

/// Compile `source_path` and write the generated Python to disk.
///
/// Layout convention:
///   * If the source is inside a `src/` directory, output goes to a sibling
///     `target/py/<stem>.py` relative to that `src/`.
///   * Otherwise, output goes alongside the source in `./target/py/<stem>.py`.
pub fn build_to_disk(source_path: &Path) -> anyhow::Result<BuildOutput> {
    let source = std::fs::read_to_string(source_path).map_err(|e| {
        anyhow::anyhow!("cannot read `{}`: {}", source_path.display(), e)
    })?;

    let config = load_corvid_config_for(source_path);
    let result = compile_with_config(&source, config.as_ref());

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
pub fn build_native_to_disk(source_path: &Path) -> anyhow::Result<NativeBuildOutput> {
    let source = std::fs::read_to_string(source_path).map_err(|e| {
        anyhow::anyhow!("cannot read `{}`: {}", source_path.display(), e)
    })?;

    let config = load_corvid_config_for(source_path);
    match compile_to_ir_with_config(&source, config.as_ref()) {
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
                corvid_codegen_cl::build_native_to_disk(&ir, &stem, &requested, &[])
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

struct FrontendBundle {
    file: corvid_ast::File,
    resolved: Resolved,
    checked: Checked,
    ir: IrFile,
    effect_registry: EffectRegistry,
}

pub fn build_target_to_disk(
    source_path: &Path,
    target: BuildTarget,
    emit_header: bool,
    emit_abi_descriptor: bool,
) -> anyhow::Result<TargetBuildOutput> {
    let source = std::fs::read_to_string(source_path).map_err(|e| {
        anyhow::anyhow!("cannot read `{}`: {}", source_path.display(), e)
    })?;

    let config = load_corvid_config_for(source_path);
    match build_frontend_bundle(&source, config.as_ref()) {
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
            let produced = match target {
                BuildTarget::Native => {
                    corvid_codegen_cl::build_native_to_disk(&frontend.ir, &stem, &requested, &[])
                }
                BuildTarget::Cdylib | BuildTarget::Staticlib => {
                    corvid_codegen_cl::build_library_to_disk(
                        &frontend.ir,
                        &stem,
                        &requested,
                        target,
                        &[],
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
                let generated_at = time::OffsetDateTime::now_utc()
                    .format(&time::format_description::well_known::Rfc3339)
                    .map_err(|e| anyhow::anyhow!("format descriptor timestamp: {e}"))?;
                let normalized_source_path = source_path.to_string_lossy().to_string();
                let descriptor = corvid_abi::emit_abi(
                    &frontend.file,
                    &frontend.resolved,
                    &frontend.checked,
                    &frontend.ir,
                    &frontend.effect_registry,
                    &corvid_abi::EmitOptions {
                        source_path: &normalized_source_path,
                        compiler_version: env!("CARGO_PKG_VERSION"),
                        generated_at: &generated_at,
                    },
                );
                let descriptor_json = corvid_abi::render_descriptor_json(&descriptor)
                    .map_err(|e| anyhow::anyhow!("serialize descriptor: {e}"))?;
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

fn build_frontend_bundle(
    source: &str,
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
    let checked = typecheck_with_config(&file, &resolved, config);
    diagnostics.extend(checked.errors.iter().cloned().map(Diagnostic::from));
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
    let ir = lower(&file, &resolved, &checked);
    Ok(FrontendBundle {
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
