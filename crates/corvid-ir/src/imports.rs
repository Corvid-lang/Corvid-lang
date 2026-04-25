//! Import-specific lowering support.
//!
//! `lower.rs` owns AST-to-IR traversal. This module owns the
//! cross-file identity problem: imported modules have their own
//! per-file `DefId` spaces, so IR lowering assigns synthetic root
//! `DefId`s before imported declarations are appended to the root IR.

use corvid_ast::{Decl, ImportSource};
use corvid_resolve::{
    remote_import_path, resolve_import_path, DeclKind, DefId, ModuleResolution, Resolved,
    ResolvedModule,
};
use corvid_types::types::ImportedStructType;
use corvid_types::Type;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ImportedDefKey {
    pub(crate) module_path: String,
    pub(crate) def_id: DefId,
}

pub(crate) fn build_imported_def_ids(
    resolved: &Resolved,
    modules: &ModuleResolution,
) -> HashMap<ImportedDefKey, DefId> {
    let mut next = resolved
        .symbols
        .entries()
        .iter()
        .map(|entry| entry.id.0)
        .max()
        .map(|max| max + 1)
        .unwrap_or(0);
    let mut out = HashMap::new();
    let mut loaded = modules.all_modules.values().collect::<Vec<_>>();
    loaded.sort_by(|a, b| a.path.cmp(&b.path));
    for module in loaded {
        let module_path = module.path.to_string_lossy().into_owned();
        for entry in module.resolved.symbols.entries() {
            if matches!(
                entry.kind,
                DeclKind::Type | DeclKind::Tool | DeclKind::Prompt | DeclKind::Agent
            ) {
                out.insert(
                    ImportedDefKey {
                        module_path: module_path.clone(),
                        def_id: entry.id,
                    },
                    DefId(next),
                );
                next += 1;
            }
        }
    }
    out
}

pub(crate) fn resolve_root_imported_type_ref(
    modules: &ModuleResolution,
    alias: &str,
    member: &str,
) -> Option<Type> {
    let module = modules.lookup(alias)?;
    let export = module.exports.get(member)?;
    if export.kind != DeclKind::Type {
        return None;
    }
    Some(Type::ImportedStruct(ImportedStructType {
        module_path: module.path.to_string_lossy().to_string(),
        def_id: export.def_id,
        name: export.name.clone(),
    }))
}

pub(crate) fn resolve_root_lifted_type_ref(
    modules: &ModuleResolution,
    name: &str,
) -> Option<Type> {
    let target = modules.lookup_imported_use(name)?;
    if target.export.kind != DeclKind::Type {
        return None;
    }
    Some(Type::ImportedStruct(ImportedStructType {
        module_path: target.module_path.to_string_lossy().to_string(),
        def_id: target.export.def_id,
        name: target.export.name.clone(),
    }))
}

pub(crate) fn resolve_module_qualified_type_ref(
    modules: &ModuleResolution,
    module: &ResolvedModule,
    imported_def_ids: &HashMap<ImportedDefKey, DefId>,
    alias: &str,
    member: &str,
) -> Option<Type> {
    let target_module = imported_module_alias_target(module, modules, alias)?;
    let export = target_module.exports.get(member)?;
    if export.kind != DeclKind::Type {
        return None;
    }
    let def_id = imported_def_ids
        .get(&ImportedDefKey {
            module_path: target_module.path.to_string_lossy().into_owned(),
            def_id: export.def_id,
        })
        .copied()
        .unwrap_or(export.def_id);
    Some(Type::Struct(def_id))
}

fn imported_module_alias_target<'a>(
    module: &ResolvedModule,
    modules: &'a ModuleResolution,
    alias: &str,
) -> Option<&'a ResolvedModule> {
    let import = module.file.decls.iter().find_map(|decl| match decl {
        Decl::Import(import)
            if matches!(
                import.source,
                ImportSource::Corvid | ImportSource::RemoteCorvid | ImportSource::PackageCorvid
            )
                && import.alias.as_ref().is_some_and(|a| a.name == alias) =>
        {
            Some(import)
        }
        _ => None,
    })?;
    let child = match import.source {
        ImportSource::Corvid => resolve_import_path(&module.path, &import.module),
        ImportSource::RemoteCorvid => remote_import_path(&import.module),
        ImportSource::PackageCorvid => remote_import_path(&import.module),
        ImportSource::Python => return None,
    };
    modules.lookup_by_path(&child)
}
