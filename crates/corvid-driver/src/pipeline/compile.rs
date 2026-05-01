//! Source-to-Python compile path.
//!
//! `compile_with_config_at_path` is the production entry point —
//! it can resolve sibling `.cor` imports because it has a real
//! file path. `compile_with_config` is the embedded variant for
//! callers that have only a string. `compile` is the no-config
//! convenience wrapper.

use std::path::Path;

use corvid_ir::lower;
use corvid_codegen_py::emit;
use corvid_syntax::{lex, parse_file};
use corvid_resolve::resolve;
use corvid_types::{typecheck_with_config, CorvidConfig};

use crate::diagnostic::Diagnostic;

use super::{lower_driver_file, typecheck_driver_file};

/// Outcome of a compile. Always contains the Python source (even
/// partial) when possible, and any diagnostics found.
pub struct CompileResult {
    pub python_source: Option<String>,
    pub diagnostics: Vec<Diagnostic>,
}

impl CompileResult {
    pub fn ok(&self) -> bool {
        self.diagnostics.is_empty()
    }
}

/// Run the full frontend on `source`. Stops collecting output when
/// errors before codegen would make it misleading.
pub fn compile(source: &str) -> CompileResult {
    compile_with_config(source, None)
}

/// Compile with an explicit `corvid.toml` configuration (for user-
/// defined effect dimensions). Callers with a source-file path
/// usually prefer `compile_with_config_at_path` which walks for
/// `corvid.toml` automatically.
pub fn compile_with_config(source: &str, config: Option<&CorvidConfig>) -> CompileResult {
    let mut diagnostics: Vec<Diagnostic> = Vec::new();

    // 1. Lex
    let tokens = match lex(source) {
        Ok(t) => t,
        Err(errs) => {
            diagnostics.extend(errs.into_iter().map(Diagnostic::from));
            return CompileResult {
                python_source: None,
                diagnostics,
            };
        }
    };

    // 2. Parse (collects errors, may still produce a partial AST)
    let (file, parse_errs) = parse_file(&tokens);
    diagnostics.extend(parse_errs.into_iter().map(Diagnostic::from));

    // 3. Resolve (collects errors)
    let resolved = resolve(&file);
    diagnostics.extend(resolved.errors.iter().cloned().map(Diagnostic::from));

    // 4. Typecheck (collects errors — this is where the killer feature lives)
    let checked = typecheck_with_config(&file, &resolved, config);
    diagnostics.extend(checked.errors.iter().cloned().map(Diagnostic::from));

    if !diagnostics.is_empty() {
        return CompileResult {
            python_source: None,
            diagnostics,
        };
    }

    // 5. Lower + 6. Codegen. Only when everything before is clean.
    let ir = lower(&file, &resolved, &checked);
    let py = emit(&ir);

    CompileResult {
        python_source: Some(py),
        diagnostics: Vec::new(),
    }
}

/// Compile a source string that came from `source_path`. Unlike
/// [`compile_with_config`], this path can resolve sibling `.cor`
/// imports because the driver still has a filesystem anchor.
pub fn compile_with_config_at_path(
    source: &str,
    source_path: &Path,
    config: Option<&CorvidConfig>,
) -> CompileResult {
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let tokens = match lex(source) {
        Ok(t) => t,
        Err(errs) => {
            diagnostics.extend(errs.into_iter().map(Diagnostic::from));
            return CompileResult {
                python_source: None,
                diagnostics,
            };
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
        return CompileResult {
            python_source: None,
            diagnostics,
        };
    }

    let ir = lower_driver_file(&file, &resolved, &typechecked.result);
    let py = emit(&ir);

    CompileResult {
        python_source: Some(py),
        diagnostics: Vec::new(),
    }
}
