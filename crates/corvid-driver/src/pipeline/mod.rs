//! Frontend pipeline entry points and the cross-cutting helpers
//! that walk the module-import graph during typecheck and adapt
//! `ModuleLoadError` into per-import diagnostics.
//!
//! Each public entry lives in its own sibling module — currently
//! just [`compile`] (Python source emit). The IR and ABI variants
//! still live in `lib.rs` and migrate here in follow-up commits.
//!
//! The shared `DriverTypecheck*` types and `typecheck_driver_file`
//! / `lower_driver_file` helpers stay here because every pipeline
//! variant needs them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use corvid_ast::{Decl, File, ImportSource, Span};
use corvid_resolve::{ModuleResolution, Resolved};
use corvid_types::{
    typecheck_with_config, typecheck_with_config_and_modules, Checked, CorvidConfig,
};

use corvid_ir::{lower, lower_with_modules, IrFile};

use crate::diagnostic::Diagnostic;
use crate::modules::{build_module_resolution, ModuleLoadError};

mod compile;
mod ir;

pub use compile::{compile, compile_with_config, compile_with_config_at_path, CompileResult};
pub use ir::{compile_to_ir, compile_to_ir_with_config, compile_to_ir_with_config_at_path};

pub(crate) struct DriverTypecheck {
    pub(crate) checked: Checked,
    pub(crate) modules: Option<ModuleResolution>,
    pub(crate) module_checked: HashMap<PathBuf, Checked>,
}

pub(crate) struct DriverTypecheckOutcome {
    pub result: DriverTypecheck,
    pub diagnostics: Vec<Diagnostic>,
}

pub(crate) fn typecheck_driver_file(
    file: &File,
    resolved: &Resolved,
    source_path: &Path,
    config: Option<&CorvidConfig>,
) -> DriverTypecheckOutcome {
    if has_corvid_imports(file) {
        let (modules, load_errors) = build_module_resolution(file, source_path);
        let diagnostics = load_errors
            .into_iter()
            .flat_map(module_load_error_diagnostics)
            .collect::<Vec<_>>();
        let checked = typecheck_with_config_and_modules(file, resolved, config, &modules);
        let (module_checked, module_diagnostics) =
            typecheck_imported_modules(&modules, config);
        let mut diagnostics = diagnostics;
        diagnostics.extend(module_diagnostics);
        DriverTypecheckOutcome {
            result: DriverTypecheck {
                checked,
                modules: Some(modules),
                module_checked,
            },
            diagnostics,
        }
    } else {
        DriverTypecheckOutcome {
            result: DriverTypecheck {
                checked: typecheck_with_config(file, resolved, config),
                modules: None,
                module_checked: HashMap::new(),
            },
            diagnostics: Vec::new(),
        }
    }
}

pub(crate) fn lower_driver_file(
    file: &File,
    resolved: &Resolved,
    typechecked: &DriverTypecheck,
) -> IrFile {
    match &typechecked.modules {
        Some(modules) => lower_with_modules(
            file,
            resolved,
            &typechecked.checked,
            modules,
            &typechecked.module_checked,
        ),
        None => lower(file, resolved, &typechecked.checked),
    }
}

fn typecheck_imported_modules(
    modules: &ModuleResolution,
    config: Option<&CorvidConfig>,
) -> (HashMap<PathBuf, Checked>, Vec<Diagnostic>) {
    let mut checked_by_path = HashMap::new();
    let mut diagnostics = Vec::new();
    let mut loaded = modules.all_modules.values().collect::<Vec<_>>();
    loaded.sort_by(|a, b| a.path.cmp(&b.path));

    for module in loaded {
        let (module_resolution, load_errors) =
            build_module_resolution(&module.file, &module.path);
        diagnostics.extend(
            load_errors
                .into_iter()
                .flat_map(module_load_error_diagnostics),
        );
        let checked = if has_corvid_imports(&module.file) {
            typecheck_with_config_and_modules(
                &module.file,
                &module.resolved,
                config,
                &module_resolution,
            )
        } else {
            typecheck_with_config(&module.file, &module.resolved, config)
        };
        diagnostics.extend(checked.errors.iter().cloned().map(|err| {
            let mut diagnostic = Diagnostic::from(err);
            diagnostic.message = format!(
                "in imported module `{}`: {}",
                module.path.display(),
                diagnostic.message
            );
            diagnostic
        }));
        checked_by_path.insert(module.path.clone(), checked);
    }

    (checked_by_path, diagnostics)
}

fn has_corvid_imports(file: &File) -> bool {
    file.decls.iter().any(|decl| {
        matches!(
            decl,
            Decl::Import(import)
                if matches!(
                    import.source,
                    ImportSource::Corvid | ImportSource::RemoteCorvid | ImportSource::PackageCorvid
                )
        )
    })
}

fn module_load_error_diagnostics(error: ModuleLoadError) -> Vec<Diagnostic> {
    let top = Span::new(0, 0);
    match error {
        ModuleLoadError::FileNotFound {
            importing_file,
            requested,
            resolved,
        } => vec![Diagnostic {
            span: top,
            message: format!(
                "import `{requested}` from `{}` could not be found at `{}`",
                importing_file.display(),
                resolved.display()
            ),
            hint: Some("check the import path relative to the importing `.cor` file".into()),
        }],
        ModuleLoadError::ReadError { path, message } => vec![Diagnostic {
            span: top,
            message: format!("failed to read imported module `{}`: {message}", path.display()),
            hint: None,
        }],
        ModuleLoadError::LexError { path, errors } => errors
            .into_iter()
            .map(|err| Diagnostic {
                span: top,
                message: format!(
                    "imported module `{}` failed to lex: {}",
                    path.display(),
                    err.kind
                ),
                hint: None,
            })
            .collect(),
        ModuleLoadError::ParseErrors { path, errors } => errors
            .into_iter()
            .map(|err| Diagnostic {
                span: top,
                message: format!(
                    "imported module `{}` failed to parse: {}",
                    path.display(),
                    err.kind
                ),
                hint: None,
            })
            .collect(),
        ModuleLoadError::HashMismatch {
            importing_file,
            requested,
            resolved,
            expected,
            actual,
        } => vec![Diagnostic {
            span: top,
            message: format!(
                "import `{requested}` from `{}` failed content-hash verification at `{}`",
                importing_file.display(),
                resolved.display()
            ),
            hint: Some(format!(
                "expected sha256:{expected}, actual sha256:{actual}; review the imported source before updating the pin"
            )),
        }],
        ModuleLoadError::RemoteImportMissingHash {
            importing_file,
            requested,
            url,
        } => vec![Diagnostic {
            span: top,
            message: format!(
                "remote import `{requested}` from `{}` is missing a content hash",
                importing_file.display()
            ),
            hint: Some(format!(
                "remote Corvid imports must use `hash:sha256:<digest>`; refusing to fetch `{url}` without a pin"
            )),
        }],
        ModuleLoadError::RemoteFetchError {
            importing_file,
            requested,
            url,
            message,
        } => vec![Diagnostic {
            span: top,
            message: format!(
                "failed to fetch remote import `{requested}` from `{}`",
                importing_file.display()
            ),
            hint: Some(format!("remote `{url}` failed before verification: {message}")),
        }],
        ModuleLoadError::PackageLockMissing {
            importing_file,
            requested,
        } => vec![Diagnostic {
            span: top,
            message: format!(
                "package import `{requested}` from `{}` is not locked",
                importing_file.display()
            ),
            hint: Some(
                "add a `Corvid.lock` entry with uri, url, and sha256 before importing packages"
                    .into(),
            ),
        }],
        ModuleLoadError::PackageLockError {
            importing_file,
            requested,
            message,
        } => vec![Diagnostic {
            span: top,
            message: format!(
                "failed to read package lock `{requested}` for `{}`",
                importing_file.display()
            ),
            hint: Some(message),
        }],
        ModuleLoadError::PackageNotLocked {
            importing_file,
            requested,
            lockfile,
        } => vec![Diagnostic {
            span: top,
            message: format!(
                "package import `{requested}` from `{}` is missing from the lockfile",
                importing_file.display()
            ),
            hint: Some(format!(
                "add `[[package]] uri = \"{requested}\"` to `{}` with a reviewed URL and sha256",
                lockfile
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "Corvid.lock".to_string())
            )),
        }],
        ModuleLoadError::Cycle { cycle } => {
            let path = cycle
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(" -> ");
            vec![Diagnostic {
                span: top,
                message: format!("cyclic Corvid import graph: {path}"),
                hint: Some(
                    "break the cycle by moving shared declarations into a third module".into(),
                ),
            }]
        }
    }
}
