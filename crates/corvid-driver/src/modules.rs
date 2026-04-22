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

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use corvid_ast::{Decl, File, ImportSource};
use corvid_resolve::{
    collect_public_exports, resolve, resolve_import_path, ModuleResolution, Resolved,
    ResolvedModule,
};
use corvid_syntax::{lex, parse_file, LexError, ParseError};

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
    let mut modules = HashMap::new();
    for import in corvid_imports(root_file) {
        let Some(alias) = import.alias.as_ref() else {
            // Aliased imports are required for `alias.Name` access.
            // A Corvid import without an alias is a no-op today;
            // the resolver's duplicate-check will still catch
            // collisions since it synthesises the module path as
            // a name. Skipping here keeps the `modules` map clean.
            continue;
        };
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
        let Some(file) = loaded.get(&loaded_path) else {
            continue;
        };
        let Some(resolved) = resolved_map.get(&loaded_path) else {
            continue;
        };
        let exports = collect_public_exports(file, resolved);
        modules.insert(
            alias.name.clone(),
            ResolvedModule {
                path: loaded_path,
                resolved: resolved.clone(),
                exports,
            },
        );
    }

    (ModuleResolution { modules }, errors)
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
