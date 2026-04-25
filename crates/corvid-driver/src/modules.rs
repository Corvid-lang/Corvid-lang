//! Driver-side loader for cross-file `.cor` imports.
//!
//! Given a root file + its path, this module walks the graph of
//! Corvid imports, parses and resolves each imported file, and
//! populates a [`ModuleResolution`] that downstream passes (the
//! type checker, in step 2c) will consult when looking up
//! `alias.Name` qualified references.
//!
//! Three passes, each with its own job:
//!
//! 1. **Collect** (`dfs_collect`) — DFS from the root, follow every
//!    Corvid import, read + parse each reachable `.cor` file.
//!    Cycle detection lives here via the classic three-color DFS:
//!    a path on the currently-processing stack that shows up as a
//!    child is a cycle.
//! 2. **Resolve** — call the existing per-file `resolve(&file)` on
//!    every loaded file. No changes to the single-file resolver.
//! 3. **Package** — build the `ModuleResolution` by pairing each
//!    alias in the ROOT file's imports with the matching loaded +
//!    resolved module, and collecting that module's public exports.
//!
//! Note the scope: only the ROOT file's direct imports populate
//! the alias→module map. Transitive imports are loaded (so cycle
//! detection across the whole graph is correct and so the checker
//! can eventually follow through type chains), but they're not
//! themselves exposed under the root's alias namespace. That
//! matches every language's import semantics — `import b; b.foo`
//! works, `b.c.bar` via transitive `b imports c` does not.
//!
//! Errors are collected rather than short-circuiting: we process
//! as much of the graph as possible so the user sees every
//! problem in a single pass.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use corvid_ast::{AgentAttribute, Decl, DimensionValue, Effect, EffectRef, File, ImportSource, TypeRef};
use corvid_resolve::{
    collect_public_exports, resolve, resolve_import_path, AgentSemanticSummary,
    ExportSemanticSummary, ImportedUseTarget, ModuleResolution, ModuleSemanticSummary, Resolved,
    ResolvedModule,
};
use corvid_syntax::{lex, parse_file, LexError, ParseError};
use corvid_types::{analyze_effects, EffectRegistry};
use anyhow::{anyhow, Context, Result};

/// Error surfaced while loading / parsing / resolving a module
/// imported from another `.cor` file. Collected rather than
/// returned per-error so the driver can surface every problem in
/// a single pass.
#[derive(Debug, Clone)]
pub enum ModuleLoadError {
    /// The imported path does not exist on disk.
    FileNotFound {
        importing_file: PathBuf,
        requested: String,
        resolved: PathBuf,
    },
    /// Reading the file from disk failed for a non-"not-found"
    /// reason (permissions, IO, etc).
    ReadError {
        path: PathBuf,
        message: String,
    },
    /// Lexing the imported file produced errors.
    LexError {
        path: PathBuf,
        errors: Vec<LexError>,
    },
    /// Parsing the imported file produced errors.
    ParseErrors {
        path: PathBuf,
        errors: Vec<ParseError>,
    },
    /// A cycle was detected in the import graph. `cycle` carries
    /// the sequence of paths from the root of the cycle back to
    /// itself (so `A -> B -> C -> A` is three entries).
    Cycle { cycle: Vec<PathBuf> },
}

#[derive(Debug, Clone)]
pub struct NamedModuleSemanticSummary {
    pub import: String,
    pub path: PathBuf,
    pub summary: ModuleSemanticSummary,
}

pub fn inspect_import_semantics(root_path: &Path) -> Result<Vec<NamedModuleSemanticSummary>> {
    let source = std::fs::read_to_string(root_path)
        .with_context(|| format!("cannot read `{}`", root_path.display()))?;
    let tokens = lex(&source).map_err(|errors| anyhow!("lex errors: {errors:?}"))?;
    let (root_file, parse_errors) = parse_file(&tokens);
    if !parse_errors.is_empty() {
        return Err(anyhow!("parse errors: {parse_errors:?}"));
    }
    let (resolution, errors) = build_module_resolution(&root_file, root_path);
    if !errors.is_empty() {
        return Err(anyhow!("module load errors: {errors:?}"));
    }
    let mut out = resolution
        .root_imports
        .iter()
        .map(|(import, module)| NamedModuleSemanticSummary {
            import: import.clone(),
            path: module.path.clone(),
            summary: module.semantic_summary.clone(),
        })
        .collect::<Vec<_>>();
    out.sort_by(|a, b| a.import.cmp(&b.import));
    Ok(out)
}

pub fn render_import_semantic_summaries(summaries: &[NamedModuleSemanticSummary]) -> String {
    if summaries.is_empty() {
        return "No Corvid imports found.\n".to_string();
    }
    let mut out = String::new();
    for module in summaries {
        out.push_str(&format!(
            "import {} -> {}\n",
            module.import,
            module.path.display()
        ));
        if module.summary.exports.is_empty() {
            out.push_str("  exports: none\n");
            continue;
        }
        for export in module.summary.exports.values() {
            out.push_str(&format!("  - {} ({:?})", export.name, export.kind));
            let mut flags = Vec::new();
            if export.deterministic {
                flags.push("deterministic".to_string());
            }
            if export.replayable {
                flags.push("replayable".to_string());
            }
            if export.approval_required {
                flags.push("approval_required".to_string());
            }
            if export.grounded_source {
                flags.push("grounded_source".to_string());
            }
            if export.grounded_return {
                flags.push("grounded_return".to_string());
            }
            if !export.effect_names.is_empty() {
                flags.push(format!("effects=[{}]", export.effect_names.join(", ")));
            }
            if let Some(agent) = module.summary.agents.get(&export.name) {
                if let Some(cost) = &agent.cost {
                    flags.push(format!("cost={}", format_dimension_value(cost)));
                }
                if !agent.violations.is_empty() {
                    flags.push(format!("violations={}", agent.violations.len()));
                }
            }
            if !flags.is_empty() {
                out.push_str(&format!(" [{}]", flags.join(", ")));
            }
            out.push('\n');
        }
    }
    out
}

/// Build a [`ModuleResolution`] for a root file located at
/// `root_path`. Returns the resolution (possibly partial) alongside
/// every error encountered.
pub fn build_module_resolution(
    root_file: &File,
    root_path: &Path,
) -> (ModuleResolution, Vec<ModuleLoadError>) {
    let mut loaded: HashMap<PathBuf, File> = HashMap::new();
    let mut in_progress: Vec<PathBuf> = Vec::new();
    let mut errors: Vec<ModuleLoadError> = Vec::new();

    // Pass 1: DFS over the import graph. The root file is already
    // parsed; we DFS into its children.
    let root_canonical = canonicalize_or_input(root_path);
    in_progress.push(root_canonical.clone());
    for import in corvid_imports(root_file) {
        let child = resolve_import_path(&root_canonical, &import.module);
        dfs_collect(
            &root_canonical,
            &child,
            &import.module,
            &mut loaded,
            &mut in_progress,
            &mut errors,
        );
    }
    in_progress.pop();

    // Pass 2: resolve each loaded file with the existing per-file
    // resolver. Errors in resolution surface through `Resolved::errors`
    // on each module, not this function's error list — single-file
    // resolution errors are the `corvid-resolve` crate's concern.
    let mut resolved_map: HashMap<PathBuf, Arc<Resolved>> = HashMap::new();
    for (path, file) in &loaded {
        resolved_map.insert(path.clone(), Arc::new(resolve(file)));
    }

    // Pass 3: package. Only the root's direct imports become
    // entries in the alias map; transitive imports are loaded
    // (for cycle detection + type-through resolution) but not
    // exposed under the root's alias namespace.
    let mut all_modules = HashMap::new();
    for (path, file) in &loaded {
        let Some(resolved) = resolved_map.get(path) else {
            continue;
        };
        let exports = collect_public_exports(file, resolved);
        let semantic_summary = build_semantic_summary(file, resolved, &exports);
        all_modules.insert(
            path.clone(),
            ResolvedModule {
                path: path.clone(),
                resolved: resolved.clone(),
                file: Arc::new(file.clone()),
                exports,
                semantic_summary,
            },
        );
    }

    let mut modules = HashMap::new();
    let mut imported_uses = HashMap::new();
    let mut root_imports = HashMap::new();
    for import in corvid_imports(root_file) {
        let import_path = resolve_import_path(&root_canonical, &import.module);
        let loaded_path = loaded
            .keys()
            .find(|p| paths_equivalent(p, &import_path))
            .cloned();
        let Some(loaded_path) = loaded_path else {
            // Loading failed earlier and errors already carry the
            // reason; skip silently here.
            continue;
        };
        let Some(module) = all_modules.get(&loaded_path).cloned() else {
            continue;
        };
        root_imports.insert(import.module.clone(), module.clone());
        if let Some(alias) = import.alias.as_ref() {
            modules.insert(alias.name.clone(), module.clone());
        }
        for item in &import.use_items {
            let lifted = item
                .alias
                .as_ref()
                .map(|alias| alias.name.clone())
                .unwrap_or_else(|| item.name.name.clone());
            if let Some(export) = module.exports.get(&item.name.name) {
                imported_uses.insert(
                    lifted,
                    ImportedUseTarget {
                        module_path: module.path.clone(),
                        export: export.clone(),
                    },
                );
            }
        }
    }

    (
        ModuleResolution {
            modules,
            imported_uses,
            root_imports,
            all_modules,
        },
        errors,
    )
}

fn build_semantic_summary(
    file: &File,
    resolved: &Resolved,
    exports: &HashMap<String, corvid_resolve::DeclExport>,
) -> ModuleSemanticSummary {
    let effect_decls: Vec<_> = file
        .decls
        .iter()
        .filter_map(|decl| match decl {
            Decl::Effect(effect) => Some(effect.clone()),
            _ => None,
        })
        .collect();
    let registry = EffectRegistry::from_decls(&effect_decls);
    let mut agent_summaries = std::collections::BTreeMap::new();
    for summary in analyze_effects(file, resolved, &registry) {
        if !exports.contains_key(&summary.agent_name) {
            continue;
        }
        let mut dimensions = std::collections::BTreeMap::new();
        for (name, value) in &summary.composed.dimensions {
            dimensions.insert(name.clone(), value.clone());
        }
        let (deterministic, replayable, grounded_return) =
            agent_flags(file, &summary.agent_name);
        let approval_required = summary
            .composed
            .dimensions
            .get("trust")
            .is_some_and(requires_approval_trust);
        let cost = summary.composed.dimensions.get("cost").cloned();
        agent_summaries.insert(
            summary.agent_name.clone(),
            AgentSemanticSummary {
                name: summary.agent_name,
                deterministic,
                replayable,
                composed_dimensions: dimensions,
                violations: summary
                    .violations
                    .iter()
                    .map(|violation| violation.to_string())
                    .collect(),
                cost,
                approval_required,
                grounded_return,
            },
        );
    }

    let mut export_summaries = std::collections::BTreeMap::new();
    for (name, export) in exports {
        if let Some(decl) = public_decl(file, name) {
            let effect_names = declared_effect_names(decl);
            let (deterministic, replayable, grounded_return) = match decl {
                Decl::Agent(agent) => (
                    AgentAttribute::is_deterministic(&agent.attributes),
                    AgentAttribute::is_replayable(&agent.attributes),
                    is_grounded_type(&agent.return_ty),
                ),
                _ => (false, false, false),
            };
            let profile = registry.compose(
                &effect_names
                    .iter()
                    .map(|effect| effect.as_str())
                    .collect::<Vec<_>>(),
            );
            let approval_required = match decl {
                Decl::Tool(tool) => {
                    matches!(tool.effect, Effect::Dangerous)
                        || profile
                            .dimensions
                            .get("trust")
                            .is_some_and(requires_approval_trust)
                }
                Decl::Agent(_) => agent_summaries
                    .get(name)
                    .is_some_and(|summary| summary.approval_required),
                _ => false,
            };
            let grounded_source = effect_names
                .iter()
                .any(|effect| effect == "retrieval")
                || profile
                    .dimensions
                    .get("data")
                    .is_some_and(|value| matches!(value, DimensionValue::Name(name) if name == "grounded"));
            export_summaries.insert(
                name.clone(),
                ExportSemanticSummary {
                    name: name.clone(),
                    kind: export.kind,
                    effect_names,
                    deterministic,
                    replayable,
                    approval_required,
                    grounded_source,
                    grounded_return,
                },
            );
        }
    }

    ModuleSemanticSummary {
        exports: export_summaries,
        agents: agent_summaries,
    }
}

fn public_decl<'a>(file: &'a File, name: &str) -> Option<&'a Decl> {
    file.decls.iter().find(|decl| match decl {
        Decl::Type(decl) => decl.name.name == name,
        Decl::Tool(decl) => decl.name.name == name,
        Decl::Prompt(decl) => decl.name.name == name,
        Decl::Agent(decl) => decl.name.name == name,
        _ => false,
    })
}

fn declared_effect_names(decl: &Decl) -> Vec<String> {
    let effects: &[EffectRef] = match decl {
        Decl::Tool(tool) => &tool.effect_row.effects,
        Decl::Prompt(prompt) => &prompt.effect_row.effects,
        Decl::Agent(agent) => &agent.effect_row.effects,
        _ => &[],
    };
    let mut names: Vec<String> = effects
        .iter()
        .map(|effect| effect.name.name.clone())
        .collect();
    if matches!(decl, Decl::Tool(tool) if matches!(tool.effect, Effect::Dangerous)) {
        names.push("dangerous".to_string());
    }
    names
}

fn agent_flags(file: &File, name: &str) -> (bool, bool, bool) {
    let Some(Decl::Agent(agent)) = public_decl(file, name) else {
        return (false, false, false);
    };
    (
        AgentAttribute::is_deterministic(&agent.attributes),
        AgentAttribute::is_replayable(&agent.attributes),
        is_grounded_type(&agent.return_ty),
    )
}

fn is_grounded_type(ty: &TypeRef) -> bool {
    matches!(ty, TypeRef::Generic { name, .. } if name.name == "Grounded")
}

fn requires_approval_trust(value: &DimensionValue) -> bool {
    matches!(value, DimensionValue::Name(name) if name == "human_required" || name == "supervisor_required")
}

fn format_dimension_value(value: &DimensionValue) -> String {
    match value {
        DimensionValue::Bool(value) => value.to_string(),
        DimensionValue::Name(value) => value.clone(),
        DimensionValue::Cost(value) => format!("${value:.4}"),
        DimensionValue::Number(value) => value.to_string(),
        DimensionValue::Streaming { backpressure } => format!("streaming({backpressure:?})"),
        DimensionValue::ConfidenceGated {
            threshold,
            above,
            below,
        } => format!("{}_if_confident({threshold:.3})_else_{}", above, below),
    }
}

/// Recursive DFS that loads the file at `target`, appends any
/// errors to `errors`, and recurses into its own Corvid imports.
/// Three-color cycle detection:
///
/// - `in_progress` carries the DFS stack. A target that's already
///   on it means a cycle.
/// - `loaded` carries fully-processed files. A target there is a
///   repeated visit and is skipped (not a cycle).
fn dfs_collect(
    importing_path: &Path,
    target: &Path,
    requested_module: &str,
    loaded: &mut HashMap<PathBuf, File>,
    in_progress: &mut Vec<PathBuf>,
    errors: &mut Vec<ModuleLoadError>,
) {
    let canonical = canonicalize_or_input(target);

    // Cycle? The DFS stack contains an earlier occurrence of this
    // path. Emit a cycle error listing the path from the earlier
    // occurrence to here.
    if let Some(idx) = in_progress.iter().position(|p| paths_equivalent(p, &canonical)) {
        let mut cycle: Vec<PathBuf> = in_progress[idx..].to_vec();
        cycle.push(canonical);
        errors.push(ModuleLoadError::Cycle { cycle });
        return;
    }

    // Already fully loaded? Not a cycle, just a repeated visit
    // (A imports B and C, both of which import D; D only loads
    // once).
    if loaded.contains_key(&canonical) {
        return;
    }

    // Load the file from disk.
    let source = match std::fs::read_to_string(&canonical) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            errors.push(ModuleLoadError::FileNotFound {
                importing_file: importing_path.to_path_buf(),
                requested: requested_module.to_string(),
                resolved: canonical,
            });
            return;
        }
        Err(e) => {
            errors.push(ModuleLoadError::ReadError {
                path: canonical,
                message: e.to_string(),
            });
            return;
        }
    };

    // Parse.
    let tokens = match lex(&source) {
        Ok(t) => t,
        Err(e) => {
            errors.push(ModuleLoadError::LexError {
                path: canonical,
                errors: e,
            });
            return;
        }
    };
    let (file, parse_errors) = parse_file(&tokens);
    if !parse_errors.is_empty() {
        errors.push(ModuleLoadError::ParseErrors {
            path: canonical.clone(),
            errors: parse_errors,
        });
        // Continue into children anyway — the partial AST may still
        // carry valid imports we want to check for cycles and load.
    }

    // Push, recurse into our own Corvid imports, pop.
    in_progress.push(canonical.clone());
    for import in corvid_imports(&file) {
        let child = resolve_import_path(&canonical, &import.module);
        dfs_collect(
            &canonical,
            &child,
            &import.module,
            loaded,
            in_progress,
            errors,
        );
    }
    in_progress.pop();

    // Mark fully loaded.
    loaded.insert(canonical, file);
}

/// Iterator over a file's Corvid imports (skipping Python imports).
fn corvid_imports(file: &File) -> impl Iterator<Item = &corvid_ast::ImportDecl> {
    file.decls.iter().filter_map(|d| match d {
        Decl::Import(i) if matches!(i.source, ImportSource::Corvid) => Some(i),
        _ => None,
    })
}

/// `std::fs::canonicalize` fails if the file doesn't exist; we
/// still want a stable, normalised path key so cycle detection
/// works even for not-yet-loaded paths. Canonicalise when we can,
/// fall back to the input path otherwise.
fn canonicalize_or_input(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Compare paths for equivalence. Canonical paths compare byte-
/// identical; non-canonical paths that equal textually compare
/// equal too. Covers the mixed case where one side is canonical
/// (loaded) and the other is not (about-to-load).
fn paths_equivalent(a: &Path, b: &Path) -> bool {
    a == b
        || std::fs::canonicalize(a)
            .ok()
            .zip(std::fs::canonicalize(b).ok())
            .map(|(ca, cb)| ca == cb)
            .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use corvid_ast::Decl;
    use std::fs;

    /// Write `contents` to `<dir>/<name>.cor`. Returns the written
    /// path.
    fn write_cor(dir: &Path, name: &str, contents: &str) -> PathBuf {
        let path = dir.join(format!("{name}.cor"));
        fs::write(&path, contents).unwrap();
        path
    }

    /// Parse a file from source. Panics on lex/parse error — the
    /// fixtures in these tests are always syntactically valid.
    fn parse_src(src: &str) -> File {
        let tokens = lex(src).expect("lex");
        let (file, errs) = parse_file(&tokens);
        assert!(errs.is_empty(), "parse errs: {errs:?}");
        file
    }

    #[test]
    fn single_import_populates_module_with_public_exports() {
        let tmp = tempfile::tempdir().unwrap();
        let root_dir = tmp.path();

        // types.cor: one public type, one private type.
        write_cor(
            root_dir,
            "types",
            "\
public type Receipt:
    ok: Bool

type Internal:
    secret: String
",
        );

        // main.cor: imports types as `t`.
        let main_src = "\
import \"./types\" as t

agent check(r: t.Receipt) -> Bool:
    return true
";
        let main_path = write_cor(root_dir, "main", main_src);
        let main_file = parse_src(main_src);

        let (res, errs) = build_module_resolution(&main_file, &main_path);

        assert!(errs.is_empty(), "unexpected module-load errors: {errs:?}");
        let module = res.lookup("t").expect("t should be resolved");
        assert!(
            module.exports.contains_key("Receipt"),
            "Receipt should be a public export: {:?}",
            module.exports.keys().collect::<Vec<_>>()
        );
        assert!(
            !module.exports.contains_key("Internal"),
            "Internal is private and must not leak: {:?}",
            module.exports.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn transitive_import_loads_but_does_not_surface_on_root_alias_map() {
        let tmp = tempfile::tempdir().unwrap();
        let root_dir = tmp.path();

        write_cor(root_dir, "leaf", "public type Leaf:\n    v: Int\n");

        write_cor(
            root_dir,
            "mid",
            "\
import \"./leaf\" as l

public type Mid:
    leaf: l.Leaf
",
        );

        let main_src = "\
import \"./mid\" as m

agent check(m0: m.Mid) -> Bool:
    return true
";
        let main_path = write_cor(root_dir, "main", main_src);
        let main_file = parse_src(main_src);

        let (res, errs) = build_module_resolution(&main_file, &main_path);

        assert!(errs.is_empty(), "unexpected errs: {errs:?}");
        assert!(res.lookup("m").is_some(), "m should be on the root alias map");
        // Transitively imported `leaf` is loaded (so cycle detection
        // is correct) but does NOT appear under the root's alias
        // namespace. That's the intended semantics.
        assert!(
            res.lookup("l").is_none(),
            "transitive import must not surface on root alias map"
        );
    }

    #[test]
    fn cycle_is_detected_with_path_list() {
        let tmp = tempfile::tempdir().unwrap();
        let root_dir = tmp.path();

        // a.cor imports b.cor imports a.cor — classic two-step cycle.
        write_cor(
            root_dir,
            "a",
            "\
import \"./b\" as b

public type A:
    v: Int
",
        );
        write_cor(
            root_dir,
            "b",
            "\
import \"./a\" as a

public type B:
    v: Int
",
        );

        let main_src = "\
import \"./a\" as a

agent check(x: a.A) -> Bool:
    return true
";
        let main_path = write_cor(root_dir, "main", main_src);
        let main_file = parse_src(main_src);

        let (_, errs) = build_module_resolution(&main_file, &main_path);

        let cycle_err = errs
            .iter()
            .find(|e| matches!(e, ModuleLoadError::Cycle { .. }))
            .expect("expected a Cycle error, got: {errs:?}");
        match cycle_err {
            ModuleLoadError::Cycle { cycle } => {
                assert!(
                    cycle.len() >= 2,
                    "cycle should list at least two paths, got: {cycle:?}"
                );
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn missing_file_produces_typed_error() {
        let tmp = tempfile::tempdir().unwrap();
        let root_dir = tmp.path();

        let main_src = "\
import \"./does_not_exist\" as dne

agent check(x: dne.Thing) -> Bool:
    return true
";
        let main_path = write_cor(root_dir, "main", main_src);
        let main_file = parse_src(main_src);

        let (res, errs) = build_module_resolution(&main_file, &main_path);

        let found = errs
            .iter()
            .any(|e| matches!(e, ModuleLoadError::FileNotFound { .. }));
        assert!(found, "expected FileNotFound, got: {errs:?}");
        assert!(
            res.lookup("dne").is_none(),
            "unresolved import must not appear in the module map"
        );
    }

    #[test]
    fn import_without_alias_is_dropped_from_modules_map() {
        let tmp = tempfile::tempdir().unwrap();
        let root_dir = tmp.path();

        write_cor(root_dir, "helpers", "public type H:\n    v: Int\n");

        let main_src = "import \"./helpers\"\n";
        let main_path = write_cor(root_dir, "main", main_src);
        let main_file = parse_src(main_src);

        let (res, errs) = build_module_resolution(&main_file, &main_path);

        assert!(errs.is_empty(), "unexpected errs: {errs:?}");
        assert!(
            res.modules.is_empty(),
            "imports without alias should be absent from the map"
        );
    }

    #[test]
    fn parse_error_in_imported_file_surfaces_with_path() {
        let tmp = tempfile::tempdir().unwrap();
        let root_dir = tmp.path();

        // Deliberately broken file (stray `@@` is not valid Corvid).
        let broken_path = root_dir.join("broken.cor");
        fs::write(&broken_path, "@@not valid corvid syntax\n").unwrap();

        let main_src = "import \"./broken\" as b\n";
        let main_path = write_cor(root_dir, "main", main_src);
        let main_file = parse_src(main_src);

        let (_, errs) = build_module_resolution(&main_file, &main_path);

        let found = errs.iter().any(|e| {
            matches!(
                e,
                ModuleLoadError::ParseErrors { .. } | ModuleLoadError::LexError { .. }
            )
        });
        assert!(
            found,
            "expected ParseErrors/LexError from broken.cor, got: {errs:?}"
        );
    }

    /// Shared imports — two files each importing the same leaf —
    /// should not produce duplicate load errors and should not be
    /// mistaken for a cycle.
    #[test]
    fn diamond_imports_do_not_report_cycle() {
        let tmp = tempfile::tempdir().unwrap();
        let root_dir = tmp.path();

        write_cor(
            root_dir,
            "leaf",
            "public type Leaf:\n    v: Int\n",
        );
        write_cor(
            root_dir,
            "left",
            "\
import \"./leaf\" as l

public type Left:
    leaf: l.Leaf
",
        );
        write_cor(
            root_dir,
            "right",
            "\
import \"./leaf\" as l

public type Right:
    leaf: l.Leaf
",
        );

        let main_src = "\
import \"./left\" as left
import \"./right\" as right

agent f(x: left.Left, y: right.Right) -> Bool:
    return true
";
        let main_path = write_cor(root_dir, "main", main_src);
        let main_file = parse_src(main_src);

        let (res, errs) = build_module_resolution(&main_file, &main_path);

        assert!(
            !errs.iter().any(|e| matches!(e, ModuleLoadError::Cycle { .. })),
            "diamond shape must not be reported as a cycle: {errs:?}"
        );
        assert!(res.lookup("left").is_some());
        assert!(res.lookup("right").is_some());
    }

    #[test]
    fn private_declaration_in_imported_file_is_not_exported() {
        let tmp = tempfile::tempdir().unwrap();
        let root_dir = tmp.path();

        // No `public` marker — file-local only.
        write_cor(
            root_dir,
            "hidden",
            "\
type Internal:
    secret: String

public type Surface:
    ok: Bool
",
        );

        let main_src = "import \"./hidden\" as h\n";
        let main_path = write_cor(root_dir, "main", main_src);
        let main_file = parse_src(main_src);

        let (res, errs) = build_module_resolution(&main_file, &main_path);
        assert!(errs.is_empty(), "errs: {errs:?}");

        let module = res.lookup("h").unwrap();
        assert!(module.exports.contains_key("Surface"));
        assert!(!module.exports.contains_key("Internal"));
    }

    /// Smoke: empty imports list produces an empty resolution
    /// cleanly and matches the default sentinel.
    #[test]
    fn no_imports_produces_empty_resolution() {
        let tmp = tempfile::tempdir().unwrap();
        let root_dir = tmp.path();

        let main_src = "agent foo(x: Int) -> Int:\n    return x\n";
        let main_path = write_cor(root_dir, "main", main_src);
        let main_file = parse_src(main_src);

        let (res, errs) = build_module_resolution(&main_file, &main_path);

        assert!(errs.is_empty());
        assert!(res.modules.is_empty());
    }

    #[test]
    fn python_imports_are_ignored_by_the_loader() {
        let tmp = tempfile::tempdir().unwrap();
        let root_dir = tmp.path();

        let main_src = "\
import python \"anthropic\" as anthropic

agent foo(x: Int) -> Int:
    return x
";
        let main_path = write_cor(root_dir, "main", main_src);
        let main_file = parse_src(main_src);

        let (res, errs) = build_module_resolution(&main_file, &main_path);
        assert!(errs.is_empty(), "python imports should not error: {errs:?}");
        assert!(res.modules.is_empty(), "only Corvid imports show up");

        // Sanity: the parser really did produce a Python import.
        let imports: Vec<_> = main_file
            .decls
            .iter()
            .filter_map(|d| match d {
                Decl::Import(i) => Some(i),
                _ => None,
            })
            .collect();
        assert_eq!(imports.len(), 1);
    }
}
