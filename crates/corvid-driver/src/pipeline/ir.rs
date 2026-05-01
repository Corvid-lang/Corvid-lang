//! Source-to-IR compile path.
//!
//! `compile_to_ir_with_config_at_path` is the production entry —
//! it walks the import graph through `typecheck_driver_file`.
//! `compile_to_ir_with_config` is the embedded variant for callers
//! that have only a string. `compile_to_ir` is the no-config
//! convenience wrapper. All three return either the lowered
//! `IrFile` or every diagnostic the frontend produced.

use std::path::Path;

use corvid_ir::{lower, IrFile};
use corvid_resolve::resolve;
use corvid_syntax::{lex, parse_file};
use corvid_types::{typecheck_with_config, CorvidConfig};

use crate::diagnostic::Diagnostic;

use super::{lower_driver_file, typecheck_driver_file};

/// Compile a source string to IR. Returns the IR or the full
/// diagnostic list.
pub fn compile_to_ir(source: &str) -> Result<IrFile, Vec<Diagnostic>> {
    compile_to_ir_with_config(source, None)
}

/// Compile a file-backed source string to IR with module-aware
/// import resolution. Production paths that have a real `.cor`
/// path should use this instead of [`compile_to_ir_with_config`].
pub fn compile_to_ir_with_config_at_path(
    source: &str,
    source_path: &Path,
    config: Option<&CorvidConfig>,
) -> Result<IrFile, Vec<Diagnostic>> {
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let tokens = match lex(source) {
        Ok(t) => t,
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
    Ok(lower_driver_file(&file, &resolved, &typechecked.result))
}

/// IR-lowering variant that consumes an explicit `corvid.toml`
/// config so user-defined effect dimensions are visible to the
/// type checker.
pub fn compile_to_ir_with_config(
    source: &str,
    config: Option<&CorvidConfig>,
) -> Result<IrFile, Vec<Diagnostic>> {
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let tokens = match lex(source) {
        Ok(t) => t,
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
    Ok(lower(&file, &resolved, &checked))
}
