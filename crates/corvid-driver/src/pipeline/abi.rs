//! Source-to-`CorvidAbi` compile path.
//!
//! `compile_to_abi_with_config` runs the full frontend (lex →
//! parse → resolve → typecheck → lower) plus the effect-registry
//! build needed by `corvid_abi::emit_abi`. Used by tools that need
//! to reason about a program's AI-safety surface (effects,
//! approval contracts, provenance, dispatch) without actually
//! emitting a library — notably `corvid trace-diff`, which
//! computes PR-level behavior deltas by comparing two
//! descriptors.

use corvid_ir::lower;
use corvid_resolve::resolve;
use corvid_syntax::{lex, parse_file};
use corvid_types::{typecheck_with_config, CorvidConfig};

use crate::diagnostic::Diagnostic;

/// Compile a source string all the way to a `CorvidAbi`
/// descriptor.
///
/// Caller-supplied `source_path_for_descriptor` goes into the
/// descriptor's `source_path` provenance field (forward-slash
/// normalised to keep Windows + Unix outputs byte-stable);
/// `generated_at` is the RFC3339 timestamp rendered into the
/// descriptor.
pub fn compile_to_abi_with_config(
    source: &str,
    source_path_for_descriptor: &str,
    generated_at: &str,
    config: Option<&CorvidConfig>,
) -> Result<corvid_abi::CorvidAbi, Vec<Diagnostic>> {
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
    let effect_decls = file
        .decls
        .iter()
        .filter_map(|decl| match decl {
            corvid_ast::Decl::Effect(effect) => Some(effect.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let effect_registry =
        corvid_types::EffectRegistry::from_decls_with_config(&effect_decls, config);
    let ir = lower(&file, &resolved, &checked);
    Ok(corvid_abi::emit_abi(
        &file,
        &resolved,
        &checked,
        &ir,
        &effect_registry,
        &corvid_abi::EmitOptions {
            source_path: source_path_for_descriptor,
            source_text: source,
            compiler_version: env!("CARGO_PKG_VERSION"),
            generated_at,
        },
    ))
}
