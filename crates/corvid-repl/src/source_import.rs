//! Source file import: load a `.cor` file's declarations into the
//! REPL session, with selective import and automatic dependency
//! resolution.

use corvid_ast::{Decl, File};
use corvid_resolve::{build_dep_graph, decl_name, resolve, DepGraph, Resolved};
use corvid_syntax::{lex, parse_file};
use std::collections::HashSet;
use std::path::Path;

/// Result of parsing and resolving a source file for import.
pub struct ParsedSource {
    pub file: File,
    pub resolved: Resolved,
    pub dep_graph: DepGraph,
    pub errors: Vec<String>,
}

/// Result of a selective import: which declarations were pulled in.
pub struct ImportResult {
    pub decls: Vec<Decl>,
    pub imported_names: Vec<String>,
    pub dependency_names: Vec<String>,
}

/// Parse and resolve a `.cor` source file.
pub fn parse_source(path: &Path) -> Result<ParsedSource, String> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read `{}`: {e}", path.display()))?;

    let tokens = lex(&source)
        .map_err(|errs| format!("lex errors in `{}`: {:?}", path.display(), errs))?;

    let (file, parse_errors) = parse_file(&tokens);
    if !parse_errors.is_empty() {
        return Err(format!(
            "parse errors in `{}`: {:?}",
            path.display(),
            parse_errors
        ));
    }

    let resolved = resolve(&file);
    let errors: Vec<String> = resolved.errors.iter().map(|e| e.to_string()).collect();
    let dep_graph = build_dep_graph(&file, &resolved);

    Ok(ParsedSource {
        file,
        resolved,
        dep_graph,
        errors,
    })
}

/// Import all declarations from a parsed source.
pub fn import_all(source: &ParsedSource) -> ImportResult {
    let mut imported_names = Vec::new();
    let decls: Vec<Decl> = source
        .file
        .decls
        .iter()
        .filter(|d| decl_name(d).is_some())
        .cloned()
        .collect();

    for d in &decls {
        if let Some(name) = decl_name(d) {
            imported_names.push(name.to_string());
        }
    }

    ImportResult {
        decls,
        imported_names,
        dependency_names: Vec::new(),
    }
}

/// Import a specific declaration by name, plus all its transitive
/// dependencies. The dep graph resolves what's needed automatically.
pub fn import_selective(
    source: &ParsedSource,
    target_name: &str,
) -> Result<ImportResult, String> {
    let target_id = source
        .resolved
        .symbols
        .lookup_def(target_name)
        .ok_or_else(|| format!("`{target_name}` is not defined in the source file"))?;

    // Collect the target + all its forward dependencies (what it needs).
    let mut needed = HashSet::new();
    needed.insert(target_id);
    collect_forward_deps(target_id, &source.dep_graph, &mut needed);

    // Map DefIds back to declarations, preserving source order.
    let mut decls = Vec::new();
    let mut imported_names = Vec::new();
    let mut dependency_names = Vec::new();

    for d in &source.file.decls {
        let name = match decl_name(d) {
            Some(n) => n,
            None => continue,
        };
        let def_id = match source.resolved.symbols.lookup_def(name) {
            Some(id) => id,
            None => continue,
        };
        if needed.contains(&def_id) {
            decls.push(d.clone());
            if def_id == target_id {
                imported_names.push(name.to_string());
            } else {
                dependency_names.push(name.to_string());
            }
        }
    }

    Ok(ImportResult {
        decls,
        imported_names,
        dependency_names,
    })
}

fn collect_forward_deps(
    id: corvid_resolve::DefId,
    graph: &DepGraph,
    collected: &mut HashSet<corvid_resolve::DefId>,
) {
    if let Some(deps) = graph.forward.get(&id) {
        for &dep in deps {
            if collected.insert(dep) {
                collect_forward_deps(dep, graph, collected);
            }
        }
    }
}
