//! Cross-file module resolution for Corvid `.cor` imports.
//!
//! When a source file contains `import "./path" as alias`, downstream
//! passes need to be able to look up `alias.Name` in the imported
//! file's exported symbol table. This module defines the shared
//! structures that carry that information across the resolver + type
//! checker boundary.
//!
//! The loader itself — which file-system walking + parsing happens
//! when — lives outside this crate so that `corvid-resolve` does not
//! depend on `corvid-syntax`. The driver populates the structures
//! here; the checker consumes them.
//!
//! The design deliberately separates three passes:
//!
//! 1. **Collect** — BFS from the root file, gather every reachable
//!    import path. Cycle detection runs here.
//! 2. **Resolve** — for each collected file, call `resolve(&file)`
//!    to produce a `Resolved`. This is the existing per-file pass,
//!    unchanged.
//! 3. **Package** — build a [`ModuleResolution`] by pairing each
//!    alias in the root's imports with its resolved module +
//!    public-export table.
//!
//! The checker receives the root file's `Resolved` plus the
//! `ModuleResolution`. When it encounters `TypeRef::Qualified`, it
//! looks up the alias → module → export. Visibility is enforced at
//! export-collection time: private declarations simply don't appear
//! in [`ResolvedModule::exports`].

use corvid_ast::{File, Field, Visibility};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::scope::{DeclKind, DefId};
use crate::Resolved;

/// A single exported top-level declaration of a module. Only
/// `Visibility::Public` (or `Visibility::PublicPackage`) decls ever
/// make it into a module's `exports` map — the private-by-default
/// rule from `lang-pub-toplevel` is enforced here, so consumers
/// can't accidentally use a file-local declaration across the
/// import boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclExport {
    pub def_id: DefId,
    pub kind: DeclKind,
    /// Name as declared; duplicated from the symbol table entry
    /// so consumers don't need to chase the def_id for trivial
    /// error messages.
    pub name: String,
    /// Struct fields for exported `type` declarations. Non-type
    /// exports leave this empty. Carrying the surface `TypeRef`s is
    /// intentional: the consumer resolves field types in the imported
    /// module's own symbol context, preserving the file boundary.
    pub type_fields: Option<Vec<Field>>,
}

/// A module referenced from the root file's `import` statements.
/// Carries a resolved view of the imported file + the set of its
/// public symbols, keyed by name for O(1) qualified-name lookup.
#[derive(Debug, Clone)]
pub struct ResolvedModule {
    /// Canonical path of the imported file on disk.
    pub path: PathBuf,
    /// Resolver output for the imported file. Held behind `Arc` so
    /// the same module can be shared if multiple files in the
    /// project import it.
    pub resolved: Arc<Resolved>,
    /// Parsed AST for the imported file. The type checker needs this
    /// to inspect struct fields by imported `DefId` without
    /// re-registering them as synthetic local declarations.
    pub file: Arc<File>,
    /// Public top-level declarations, indexed by name. Private
    /// declarations are deliberately absent — the visibility check
    /// is enforced at insertion time, not at lookup time, so we
    /// cannot accidentally leak a private binding by forgetting to
    /// check visibility on the consumer side.
    pub exports: HashMap<String, DeclExport>,
}

/// A public declaration lifted into the importing file's unqualified
/// namespace by `import "./path" use Name, Other as Alias`.
#[derive(Debug, Clone)]
pub struct ImportedUseTarget {
    pub module_path: PathBuf,
    pub export: DeclExport,
}

/// The module graph for a single root compilation. Maps each
/// `import ... as alias` in the root file to its `ResolvedModule`.
/// Aliases are unique within a file (enforced by the resolver's
/// duplicate-decl check), so the `HashMap` is safe to key on
/// alias name.
///
/// When the root file has no Corvid imports, `modules` is empty
/// and the checker proceeds with single-file semantics unchanged.
#[derive(Debug, Clone, Default)]
pub struct ModuleResolution {
    pub modules: HashMap<String, ResolvedModule>,
    pub imported_uses: HashMap<String, ImportedUseTarget>,
    /// Every loaded module keyed by canonical-ish path, including
    /// transitive imports. The root alias map above controls what
    /// names the root file may use; this path map lets the checker
    /// resolve field types inside an imported module's own context.
    pub all_modules: HashMap<PathBuf, ResolvedModule>,
}

impl ModuleResolution {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn lookup(&self, alias: &str) -> Option<&ResolvedModule> {
        self.modules.get(alias)
    }

    pub fn lookup_by_path(&self, path: &Path) -> Option<&ResolvedModule> {
        self.all_modules.get(path).or_else(|| {
            self.all_modules
                .iter()
                .find_map(|(p, module)| if p == path { Some(module) } else { None })
        })
    }

    pub fn lookup_member(&self, alias: &str, name: &str) -> ModuleLookup<'_> {
        match self.modules.get(alias) {
            None => ModuleLookup::UnknownAlias,
            Some(module) => match module.exports.get(name) {
                Some(export) => ModuleLookup::Found { module, export },
                None => {
                    // The member didn't resolve as a public export.
                    // Distinguish "exists but private" (the file
                    // declared it without `pub`) from "doesn't exist
                    // at all" for a better error message.
                    let is_private = module
                        .resolved
                        .symbols
                        .lookup_def(name)
                        .is_some();
                    if is_private {
                        ModuleLookup::Private
                    } else {
                        ModuleLookup::UnknownMember
                    }
                }
            },
        }
    }

    pub fn lookup_imported_use(&self, name: &str) -> Option<&ImportedUseTarget> {
        self.imported_uses.get(name)
    }
}

/// The three outcomes of `alias.Name` lookup. Each maps naturally
/// to a typed error message the checker can surface.
#[derive(Debug)]
pub enum ModuleLookup<'a> {
    Found {
        module: &'a ResolvedModule,
        export: &'a DeclExport,
    },
    /// The alias itself didn't refer to a known import.
    UnknownAlias,
    /// The imported file has a declaration named `name`, but it's
    /// private (no `public` / `public(package)` modifier). The
    /// visibility check is the whole point of `lang-pub-toplevel`.
    Private,
    /// The imported file has no top-level declaration called `name`,
    /// publicly or privately.
    UnknownMember,
}

/// Collect the public-export map for an already-resolved file.
/// Called by the driver's module-loading pass once each module is
/// fully resolved. Skips any declaration whose `Visibility` is
/// `Private`, so the resulting map only contains names that are
/// legitimately importable.
pub fn collect_public_exports(file: &File, resolved: &Resolved) -> HashMap<String, DeclExport> {
    let mut out = HashMap::new();
    for decl in &file.decls {
        let (name, visibility, kind, type_fields) = match decl {
            corvid_ast::Decl::Type(t) => (
                t.name.name.as_str(),
                t.visibility,
                DeclKind::Type,
                Some(t.fields.clone()),
            ),
            corvid_ast::Decl::Tool(t) => (t.name.name.as_str(), t.visibility, DeclKind::Tool, None),
            corvid_ast::Decl::Prompt(p) => {
                (p.name.name.as_str(), p.visibility, DeclKind::Prompt, None)
            }
            corvid_ast::Decl::Agent(a) => {
                (a.name.name.as_str(), a.visibility, DeclKind::Agent, None)
            }
            // Imports, effects, models, evals, extends don't carry
            // module-level visibility today. `extend` methods do, but
            // those ride on the underlying type's visibility and
            // aren't top-level exports themselves.
            _ => continue,
        };
        if matches!(visibility, Visibility::Private) {
            continue;
        }
        if let Some(def_id) = resolved.symbols.lookup_def(name) {
            out.insert(
                name.to_string(),
                DeclExport {
                    def_id,
                    kind,
                    name: name.to_string(),
                    type_fields,
                },
            );
        }
    }
    out
}

/// Canonicalize an import path relative to the importing file.
/// `./foo` becomes `<dir>/foo.cor`; `../bar/baz` becomes the
/// corresponding absolute path. The `.cor` extension is implicit
/// in the Corvid import syntax, so this helper adds it when the
/// user wrote a bare name.
pub fn resolve_import_path(importing_file: &Path, module: &str) -> PathBuf {
    let base = importing_file
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();
    let rel = Path::new(module);
    let mut candidate = base.join(rel);
    if candidate.extension().is_none() {
        candidate.set_extension("cor");
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_import_path_appends_cor_extension() {
        let base = Path::new("/proj/src/main.cor");
        let resolved = resolve_import_path(base, "./default_policy");
        assert_eq!(resolved, PathBuf::from("/proj/src/./default_policy.cor"));
    }

    #[test]
    fn resolve_import_path_preserves_explicit_extension() {
        let base = Path::new("/proj/src/main.cor");
        let resolved = resolve_import_path(base, "./types.cor");
        assert_eq!(resolved, PathBuf::from("/proj/src/./types.cor"));
    }

    #[test]
    fn resolve_import_path_supports_parent_dir() {
        let base = Path::new("/proj/src/policies/team.cor");
        let resolved = resolve_import_path(base, "../shared/types");
        assert_eq!(
            resolved,
            PathBuf::from("/proj/src/policies/../shared/types.cor")
        );
    }

    #[test]
    fn empty_module_resolution_is_useful_sentinel() {
        let r = ModuleResolution::empty();
        assert!(r.modules.is_empty());
        assert!(matches!(r.lookup_member("p", "Foo"), ModuleLookup::UnknownAlias));
    }
}
