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
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::import_integrity::sha256_hex;
use crate::package_lock::{load_package_lock_for, PackageLockFile};
use anyhow::{anyhow, Context, Result};
use corvid_ast::{
    AgentAttribute, Decl, DimensionValue, Effect, EffectRef, File, ImportContentHash,
    ImportSource, TypeRef,
};
use corvid_resolve::{
    collect_public_exports, remote_import_path, resolve, resolve_import_path, AgentSemanticSummary,
    ExportSemanticSummary, ImportedUseTarget, ModuleResolution, ModuleSemanticSummary, Resolved,
    ResolvedModule,
};
use corvid_syntax::{lex, parse_file, LexError, ParseError};
use corvid_types::{analyze_effects, EffectRegistry};
use url::Url;

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
    /// A pinned import read successfully, but its bytes do not match
    /// the import's declared SHA-256 digest.
    HashMismatch {
        importing_file: PathBuf,
        requested: String,
        resolved: PathBuf,
        expected: String,
        actual: String,
    },
    /// A remote import was presented to the loader without a content
    /// hash. The parser rejects this in normal source files; the
    /// loader keeps the invariant for programmatically constructed ASTs.
    RemoteImportMissingHash {
        importing_file: PathBuf,
        requested: String,
        url: String,
    },
    /// Fetching a remote Corvid import failed before hash verification.
    RemoteFetchError {
        importing_file: PathBuf,
        requested: String,
        url: String,
        message: String,
    },
    /// A package import needs `Corvid.lock`, but no lockfile was found
    /// by walking up from the root source file.
    PackageLockMissing {
        importing_file: PathBuf,
        requested: String,
    },
    /// The lockfile existed but could not be read or parsed.
    PackageLockError {
        importing_file: PathBuf,
        requested: String,
        message: String,
    },
    /// A `corvid://...` import was not present in `Corvid.lock`.
    PackageNotLocked {
        importing_file: PathBuf,
        requested: String,
        lockfile: Option<PathBuf>,
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
    pub content_hash: Option<ImportContentHash>,
    pub summary: ModuleSemanticSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ImportTarget {
    Local(PathBuf),
    Remote { url: String, key: PathBuf },
}

#[derive(Debug, Clone)]
struct ResolvedImportTarget {
    target: ImportTarget,
    content_hash: Option<ImportContentHash>,
}

impl ImportTarget {
    fn local(path: PathBuf) -> Self {
        Self::Local(path)
    }

    fn remote(url: String) -> Self {
        Self::Remote {
            key: remote_import_path(&url),
            url,
        }
    }

    fn key(&self) -> PathBuf {
        match self {
            Self::Local(path) => path.clone(),
            Self::Remote { key, .. } => key.clone(),
        }
    }

    fn display(&self) -> String {
        match self {
            Self::Local(path) => path.display().to_string(),
            Self::Remote { url, .. } => url.clone(),
        }
    }
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
    let mut out = corvid_imports(&root_file)
        .filter_map(|import| {
            resolution
                .root_imports
                .get(&import.module)
                .map(|module| NamedModuleSemanticSummary {
                    import: import.module.clone(),
                    path: module.path.clone(),
                    content_hash: import.content_hash.clone(),
                    summary: module.semantic_summary.clone(),
                })
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
            "import {}{} -> {}\n",
            module.import,
            module
                .content_hash
                .as_ref()
                .map(|hash| format!(" hash:{}:{}", hash.algorithm, hash.hex))
                .unwrap_or_default(),
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
    let root_target = ImportTarget::local(root_canonical.clone());
    let package_lock = match load_package_lock_for(root_path) {
        Ok(lock) => lock,
        Err(message) => {
            errors.push(ModuleLoadError::PackageLockError {
                importing_file: root_target.key(),
                requested: "Corvid.lock".to_string(),
                message,
            });
            None
        }
    };
    in_progress.push(root_target.key());
    for import in corvid_imports(root_file) {
        match resolve_child_target(&root_target, import, package_lock.as_ref()) {
            Ok(child) => dfs_collect(
                &root_target,
                child.target,
                &import.module,
                child.content_hash.as_ref(),
                package_lock.as_ref(),
                &mut loaded,
                &mut in_progress,
                &mut errors,
            ),
            Err(error) => errors.push(error),
        }
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
        let loaded_path = resolve_child_target(&root_target, import, package_lock.as_ref())
            .ok()
            .map(|resolved| resolved.target.key())
            .and_then(|import_path| {
                loaded
                    .keys()
                    .find(|p| paths_equivalent(p, &import_path))
                    .cloned()
            });
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

pub(crate) fn summarize_module_source(source: &str) -> Result<ModuleSemanticSummary, String> {
    let tokens = lex(source).map_err(|errors| format!("lex errors: {errors:?}"))?;
    let (file, parse_errors) = parse_file(&tokens);
    if !parse_errors.is_empty() {
        return Err(format!("parse errors: {parse_errors:?}"));
    }
    let resolved = resolve(&file);
    if !resolved.errors.is_empty() {
        return Err(format!("resolve errors: {:?}", resolved.errors));
    }
    let exports = collect_public_exports(&file, &resolved);
    Ok(build_semantic_summary(&file, &resolved, &exports))
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

fn resolve_child_target(
    parent: &ImportTarget,
    import: &corvid_ast::ImportDecl,
    package_lock: Option<&PackageLockFile>,
) -> Result<ResolvedImportTarget, ModuleLoadError> {
    match import.source {
        ImportSource::Corvid => match parent {
            ImportTarget::Local(path) => {
                Ok(ResolvedImportTarget {
                    target: ImportTarget::local(resolve_import_path(path, &import.module)),
                    content_hash: import.content_hash.clone(),
                })
            }
            ImportTarget::Remote { url, .. } => {
                let resolved = resolve_remote_url(url, &import.module).map_err(|message| {
                    ModuleLoadError::RemoteFetchError {
                        importing_file: parent.key(),
                        requested: import.module.clone(),
                        url: url.clone(),
                        message,
                    }
                })?;
                Ok(ResolvedImportTarget {
                    target: ImportTarget::remote(resolved),
                    content_hash: import.content_hash.clone(),
                })
            }
        },
        ImportSource::RemoteCorvid => Ok(ResolvedImportTarget {
            target: ImportTarget::remote(import.module.clone()),
            content_hash: import.content_hash.clone(),
        }),
        ImportSource::PackageCorvid => {
            let Some(lockfile) = package_lock else {
                return Err(ModuleLoadError::PackageLockMissing {
                    importing_file: parent.key(),
                    requested: import.module.clone(),
                });
            };
            let Some(entry) = lockfile.lock.find(&import.module) else {
                return Err(ModuleLoadError::PackageNotLocked {
                    importing_file: parent.key(),
                    requested: import.module.clone(),
                    lockfile: Some(lockfile.path.clone()),
                });
            };
            Ok(ResolvedImportTarget {
                target: ImportTarget::remote(entry.url.clone()),
                content_hash: Some(ImportContentHash {
                    algorithm: "sha256".to_string(),
                    hex: entry.sha256.to_ascii_lowercase(),
                    span: import.span,
                }),
            })
        }
        ImportSource::Python => unreachable!("python imports are filtered before module loading"),
    }
}

fn load_target_bytes(
    importing_target: &ImportTarget,
    target: &ImportTarget,
    requested_module: &str,
    expected_hash: Option<&ImportContentHash>,
) -> Result<Vec<u8>, ModuleLoadError> {
    match target {
        ImportTarget::Local(path) => match std::fs::read(path) {
            Ok(bytes) => Ok(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(ModuleLoadError::FileNotFound {
                    importing_file: importing_target.key(),
                    requested: requested_module.to_string(),
                    resolved: path.clone(),
                })
            }
            Err(e) => Err(ModuleLoadError::ReadError {
                path: path.clone(),
                message: e.to_string(),
            }),
        },
        ImportTarget::Remote { url, .. } => {
            if expected_hash.is_none() {
                return Err(ModuleLoadError::RemoteImportMissingHash {
                    importing_file: importing_target.key(),
                    requested: requested_module.to_string(),
                    url: url.clone(),
                });
            }
            fetch_remote_bytes(url).map_err(|message| ModuleLoadError::RemoteFetchError {
                importing_file: importing_target.key(),
                requested: requested_module.to_string(),
                url: url.clone(),
                message,
            })
        }
    }
}

fn fetch_remote_bytes(url: &str) -> Result<Vec<u8>, String> {
    let response = ureq::get(url)
        .call()
        .map_err(|err| err.to_string())?;
    if !(200..=299).contains(&response.status()) {
        return Err(format!("HTTP status {}", response.status()));
    }
    let mut bytes = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|err| err.to_string())?;
    Ok(bytes)
}

fn resolve_remote_url(base: &str, module: &str) -> Result<String, String> {
    if module.starts_with("https://") || module.starts_with("http://") {
        return Ok(module.to_string());
    }
    let base = Url::parse(base).map_err(|err| err.to_string())?;
    let module = if module.ends_with(".cor") {
        module.to_string()
    } else {
        format!("{module}.cor")
    };
    base.join(&module)
        .map(|url| url.to_string())
        .map_err(|err| err.to_string())
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
    importing_target: &ImportTarget,
    target: ImportTarget,
    requested_module: &str,
    expected_hash: Option<&ImportContentHash>,
    package_lock: Option<&PackageLockFile>,
    loaded: &mut HashMap<PathBuf, File>,
    in_progress: &mut Vec<PathBuf>,
    errors: &mut Vec<ModuleLoadError>,
) {
    let canonical = target.key();

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

    // Load and verify the module source. Hash verification runs on
    // the exact bytes before parsing, so a pinned import cannot drift
    // into the compiler under a stale trust boundary.
    let bytes = match load_target_bytes(importing_target, &target, requested_module, expected_hash) {
        Ok(bytes) => bytes,
        Err(error) => {
            errors.push(error);
            return;
        }
    };
    if let Some(expected_hash) = expected_hash {
        let actual = sha256_hex(&bytes);
        if actual != expected_hash.hex {
            errors.push(ModuleLoadError::HashMismatch {
                importing_file: importing_target.key(),
                requested: requested_module.to_string(),
                resolved: canonical,
                expected: expected_hash.hex.clone(),
                actual,
            });
            return;
        }
    }
    let source = match String::from_utf8(bytes) {
        Ok(source) => source,
        Err(err) => {
            errors.push(ModuleLoadError::ReadError {
                path: canonical,
                message: format!("imported module is not valid UTF-8: {err}"),
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
        match resolve_child_target(&target, import, package_lock) {
            Ok(child) => dfs_collect(
                &target,
                child.target,
                &import.module,
                child.content_hash.as_ref(),
                package_lock,
                loaded,
                in_progress,
                errors,
            ),
            Err(error) => errors.push(error),
        }
    }
    in_progress.pop();

    // Mark fully loaded.
    loaded.insert(canonical, file);
}

/// Iterator over a file's Corvid imports (skipping Python imports).
fn corvid_imports(file: &File) -> impl Iterator<Item = &corvid_ast::ImportDecl> {
    file.decls.iter().filter_map(|d| match d {
        Decl::Import(i)
            if matches!(
                i.source,
                ImportSource::Corvid | ImportSource::RemoteCorvid | ImportSource::PackageCorvid
            ) =>
        {
            Some(i)
        }
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
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

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

    fn serve_once(path: &'static str, body: impl Into<String>) -> String {
        let body = body.into();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let n = stream.read(&mut request).unwrap_or(0);
            let request = String::from_utf8_lossy(&request[..n]);
            let status = if request.starts_with(&format!("GET {path} ")) {
                "HTTP/1.1 200 OK"
            } else {
                "HTTP/1.1 404 Not Found"
            };
            let body = if status.contains("200") {
                body.as_str()
            } else {
                "not found"
            };
            write!(
                stream,
                "{status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.as_bytes().len(),
                body
            )
            .unwrap();
        });
        format!("http://{addr}{path}")
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
import python \"anthropic\" as anthropic effects: network

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

    #[test]
    fn hash_pinned_import_loads_when_digest_matches() {
        let tmp = tempfile::tempdir().unwrap();
        let root_dir = tmp.path();
        let module_src = "public type Policy:\n    ok: Bool\n";
        write_cor(root_dir, "policy", module_src);
        let digest = crate::import_integrity::sha256_hex(module_src.as_bytes());
        let main_src = format!(
            "\
import \"./policy\" hash:sha256:{digest} as p

agent check(x: p.Policy) -> Bool:
    return true
"
        );
        let main_path = write_cor(root_dir, "main", &main_src);
        let main_file = parse_src(&main_src);

        let (res, errs) = build_module_resolution(&main_file, &main_path);

        assert!(errs.is_empty(), "unexpected errs: {errs:?}");
        assert!(res.lookup("p").is_some());
    }

    #[test]
    fn hash_pinned_import_fails_closed_when_digest_mismatches() {
        let tmp = tempfile::tempdir().unwrap();
        let root_dir = tmp.path();
        write_cor(root_dir, "policy", "public type Policy:\n    ok: Bool\n");
        let wrong = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let main_src = format!(
            "\
import \"./policy\" hash:sha256:{wrong} as p

agent check() -> Bool:
    return true
"
        );
        let main_path = write_cor(root_dir, "main", &main_src);
        let main_file = parse_src(&main_src);

        let (res, errs) = build_module_resolution(&main_file, &main_path);

        assert!(
            errs.iter()
                .any(|err| matches!(err, ModuleLoadError::HashMismatch { .. })),
            "expected HashMismatch, got {errs:?}"
        );
        assert!(
            res.lookup("p").is_none(),
            "mismatched module must not enter alias map"
        );
    }

    #[test]
    fn remote_hash_pinned_import_loads_when_digest_matches() {
        let tmp = tempfile::tempdir().unwrap();
        let root_dir = tmp.path();
        let module_src = "public type RemotePolicy:\n    ok: Bool\n";
        let digest = crate::import_integrity::sha256_hex(module_src.as_bytes());
        let url = serve_once("/policy.cor", module_src);
        let main_src = format!(
            "\
import \"{url}\" hash:sha256:{digest} as p

agent check(x: p.RemotePolicy) -> Bool:
    return true
"
        );
        let main_path = write_cor(root_dir, "main", &main_src);
        let main_file = parse_src(&main_src);

        let (res, errs) = build_module_resolution(&main_file, &main_path);

        assert!(errs.is_empty(), "unexpected errs: {errs:?}");
        assert!(res.lookup("p").is_some());
    }

    #[test]
    fn remote_hash_pinned_import_fails_closed_when_digest_mismatches() {
        let tmp = tempfile::tempdir().unwrap();
        let root_dir = tmp.path();
        let url = serve_once("/policy.cor", "public type RemotePolicy:\n    ok: Bool\n");
        let wrong = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let main_src = format!(
            "\
import \"{url}\" hash:sha256:{wrong} as p

agent check() -> Bool:
    return true
"
        );
        let main_path = write_cor(root_dir, "main", &main_src);
        let main_file = parse_src(&main_src);

        let (res, errs) = build_module_resolution(&main_file, &main_path);

        assert!(
            errs.iter()
                .any(|err| matches!(err, ModuleLoadError::HashMismatch { .. })),
            "expected HashMismatch, got {errs:?}"
        );
        assert!(res.lookup("p").is_none());
    }

    #[test]
    fn package_import_resolves_through_lockfile_and_hash_verifies() {
        let tmp = tempfile::tempdir().unwrap();
        let root_dir = tmp.path();
        let package_src = "\
public type SafetyReceipt:
    id: String
";
        let digest = sha256_hex(package_src.as_bytes());
        let url = serve_once("/safety-baseline-v2.3.cor", package_src);
        fs::write(
            root_dir.join("Corvid.lock"),
            format!(
                "\
[[package]]
uri = \"corvid://@anthropic/safety-baseline/v2.3\"
url = \"{url}\"
sha256 = \"{digest}\"
registry = \"https://registry.corvid.dev\"
signature = \"unsigned:test-fixture\"
"
            ),
        )
        .unwrap();
        let main_src = "\
import \"corvid://@anthropic/safety-baseline/v2.3\" as safety

agent check(r: safety.SafetyReceipt) -> String:
    return r.id
";
        let main_path = write_cor(root_dir, "main", main_src);
        let main_file = parse_src(main_src);

        let (res, errs) = build_module_resolution(&main_file, &main_path);

        assert!(errs.is_empty(), "unexpected package-load errors: {errs:?}");
        let module = res.lookup("safety").expect("package alias");
        assert!(module.exports.contains_key("SafetyReceipt"));
    }

    #[test]
    fn package_import_without_lockfile_fails_closed() {
        let tmp = tempfile::tempdir().unwrap();
        let root_dir = tmp.path();
        let main_src = r#"import "corvid://@anthropic/safety-baseline/v2.3" as safety"#;
        let main_path = write_cor(root_dir, "main", main_src);
        let main_file = parse_src(main_src);

        let (_res, errs) = build_module_resolution(&main_file, &main_path);

        assert!(errs.iter().any(|err| matches!(
            err,
            ModuleLoadError::PackageLockMissing { requested, .. }
                if requested == "corvid://@anthropic/safety-baseline/v2.3"
        )));
    }

    #[test]
    fn package_import_lock_hash_mismatch_fails_closed() {
        let tmp = tempfile::tempdir().unwrap();
        let root_dir = tmp.path();
        let package_src = "public type SafetyReceipt:\n    id: String\n";
        let url = serve_once("/safety-baseline-v2.3.cor", package_src);
        fs::write(
            root_dir.join("Corvid.lock"),
            format!(
                "\
[[package]]
uri = \"corvid://@anthropic/safety-baseline/v2.3\"
url = \"{url}\"
sha256 = \"0000000000000000000000000000000000000000000000000000000000000000\"
"
            ),
        )
        .unwrap();
        let main_src = r#"import "corvid://@anthropic/safety-baseline/v2.3" as safety"#;
        let main_path = write_cor(root_dir, "main", main_src);
        let main_file = parse_src(main_src);

        let (_res, errs) = build_module_resolution(&main_file, &main_path);

        assert!(errs.iter().any(|err| matches!(
            err,
            ModuleLoadError::HashMismatch { requested, .. }
                if requested == "corvid://@anthropic/safety-baseline/v2.3"
        )));
    }
}
