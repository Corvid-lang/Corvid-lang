//! Independent ABI descriptor verifier.
//!
//! This crate deliberately does not call `corvid-driver`'s build
//! helpers. It rebuilds the descriptor through the descriptor-relevant
//! frontend steps only: lex, parse, resolve, typecheck, IR lower, ABI
//! emit. The produced descriptor JSON is then byte-compared with the
//! `CORVID_ABI_DESCRIPTOR` symbol embedded in a cdylib.

use anyhow::{anyhow, bail, Context, Result};
use corvid_abi::{
    hash_json_str, normalize_source_path, read_embedded_section_from_library,
    render_descriptor_json, EmitOptions,
};
use corvid_ast::{Decl, ImportSource};
use corvid_driver::build_module_resolution;
use corvid_ir::{lower, lower_with_modules};
use corvid_resolve::{resolve, ModuleResolution};
use corvid_syntax::{lex, parse_file};
use corvid_types::{
    typecheck_with_config, typecheck_with_config_and_modules, Checked, CorvidConfig, EffectRegistry,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbiVerifyReport {
    pub source_json_hash: [u8; 32],
    pub embedded_json_hash: [u8; 32],
    pub source_json_len: usize,
    pub embedded_json_len: usize,
}

impl AbiVerifyReport {
    pub fn matches(&self) -> bool {
        self.source_json_hash == self.embedded_json_hash
            && self.source_json_len == self.embedded_json_len
    }
}

pub fn verify_source_matches_cdylib(
    source_path: &Path,
    cdylib_path: &Path,
) -> Result<AbiVerifyReport> {
    let source_json = rebuild_descriptor_json(source_path)?;
    let embedded = read_embedded_section_from_library(cdylib_path).with_context(|| {
        format!(
            "read `{}` symbol from `{}`",
            corvid_abi::CORVID_ABI_DESCRIPTOR_SYMBOL,
            cdylib_path.display()
        )
    })?;

    let report = AbiVerifyReport {
        source_json_hash: hash_json_str(&source_json),
        embedded_json_hash: hash_json_str(&embedded.json),
        source_json_len: source_json.len(),
        embedded_json_len: embedded.json.len(),
    };

    if source_json.as_bytes() != embedded.json.as_bytes() {
        bail!(
            "ABI descriptor mismatch: rebuilt descriptor from `{}` does not byte-match embedded descriptor in `{}`",
            source_path.display(),
            cdylib_path.display()
        );
    }

    Ok(report)
}

pub fn rebuild_descriptor_json(source_path: &Path) -> Result<String> {
    let source = std::fs::read_to_string(source_path)
        .with_context(|| format!("read Corvid source `{}`", source_path.display()))?;
    let source_dir = source_path.parent().unwrap_or_else(|| Path::new("."));
    let config = CorvidConfig::load_walking(source_dir)
        .with_context(|| format!("load corvid.toml for `{}`", source_path.display()))?;

    let tokens = lex(&source).map_err(|errors| anyhow!("lex errors: {errors:?}"))?;
    let (file, parse_errors) = parse_file(&tokens);
    if !parse_errors.is_empty() {
        bail!("parse errors: {parse_errors:?}");
    }

    let resolved = resolve(&file);
    if !resolved.errors.is_empty() {
        bail!("resolve errors: {:?}", resolved.errors);
    }

    let modules = if has_corvid_imports(&file) {
        let (modules, load_errors) = build_module_resolution(&file, source_path);
        if !load_errors.is_empty() {
            bail!("module load errors: {load_errors:?}");
        }
        Some(modules)
    } else {
        None
    };

    let checked = match &modules {
        Some(modules) => {
            typecheck_with_config_and_modules(&file, &resolved, config.as_ref(), modules)
        }
        None => typecheck_with_config(&file, &resolved, config.as_ref()),
    };
    if !checked.errors.is_empty() {
        bail!("typecheck errors: {:?}", checked.errors);
    }
    let module_checked = match &modules {
        Some(modules) => typecheck_imported_modules(modules, config.as_ref())?,
        None => HashMap::new(),
    };

    let effect_decls = file
        .decls
        .iter()
        .filter_map(|decl| match decl {
            Decl::Effect(effect) => Some(effect.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let effect_registry = EffectRegistry::from_decls_with_config(&effect_decls, config.as_ref());
    let ir = match &modules {
        Some(modules) => lower_with_modules(&file, &resolved, &checked, modules, &module_checked),
        None => lower(&file, &resolved, &checked),
    };
    let generated_at = "1970-01-01T00:00:00Z".to_string();
    let normalized_source_path = normalize_source_path(&source_path.to_string_lossy());
    let descriptor = corvid_abi::emit_catalog_abi(
        &file,
        &resolved,
        &checked,
        &ir,
        &effect_registry,
        &EmitOptions {
            source_path: &normalized_source_path,
            source_text: &source,
            compiler_version: env!("CARGO_PKG_VERSION"),
            generated_at: &generated_at,
        },
    );
    render_descriptor_json(&descriptor).context("serialize rebuilt ABI descriptor")
}

fn typecheck_imported_modules(
    modules: &ModuleResolution,
    config: Option<&CorvidConfig>,
) -> Result<HashMap<PathBuf, Checked>> {
    let mut checked_by_path = HashMap::new();
    let mut loaded = modules.all_modules.values().collect::<Vec<_>>();
    loaded.sort_by(|a, b| a.path.cmp(&b.path));

    for module in loaded {
        let nested_modules = if has_corvid_imports(&module.file) {
            let (nested, load_errors) = build_module_resolution(&module.file, &module.path);
            if !load_errors.is_empty() {
                bail!(
                    "module load errors while checking imported module `{}`: {load_errors:?}",
                    module.path.display()
                );
            }
            Some(nested)
        } else {
            None
        };
        let checked = match &nested_modules {
            Some(nested) => {
                typecheck_with_config_and_modules(&module.file, &module.resolved, config, nested)
            }
            None => typecheck_with_config(&module.file, &module.resolved, config),
        };
        if !checked.errors.is_empty() {
            bail!(
                "typecheck errors in imported module `{}`: {:?}",
                module.path.display(),
                checked.errors
            );
        }
        checked_by_path.insert(module.path.clone(), checked);
    }

    Ok(checked_by_path)
}

fn has_corvid_imports(file: &corvid_ast::File) -> bool {
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

pub fn hex_hash(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use corvid_driver::{build_target_to_disk, BuildTarget};

    const SOURCE: &str = r#"
pub extern "c"
agent answer(x: Int) -> Int:
    return x + 1
"#;

    #[test]
    fn verifier_accepts_matching_cdylib_descriptor() {
        let temp = tempfile::tempdir().expect("tempdir");
        let src = temp.path().join("answer.cor");
        std::fs::write(&src, SOURCE).expect("write source");
        let build = build_target_to_disk(&src, BuildTarget::Cdylib, false, false, &[], None)
            .expect("build matching cdylib");
        assert!(build.diagnostics.is_empty(), "{:?}", build.diagnostics);
        let cdylib = build.output_path.expect("cdylib path");

        let report = verify_source_matches_cdylib(&src, &cdylib).expect("verify");
        assert!(report.matches(), "report: {report:?}");
    }

    #[test]
    fn verifier_rejects_source_descriptor_mismatch() {
        let temp = tempfile::tempdir().expect("tempdir");
        let src = temp.path().join("answer.cor");
        std::fs::write(&src, SOURCE).expect("write source");
        let build = build_target_to_disk(&src, BuildTarget::Cdylib, false, false, &[], None)
            .expect("build matching cdylib");
        assert!(build.diagnostics.is_empty(), "{:?}", build.diagnostics);
        let cdylib = build.output_path.expect("cdylib path");
        std::fs::write(
            &src,
            r#"
pub extern "c"
agent answer(x: Int, y: Int) -> Int:
    return x + y
"#,
        )
        .expect("mutate source");

        let err = verify_source_matches_cdylib(&src, &cdylib).expect_err("mismatch must fail");
        assert!(
            err.to_string().contains("ABI descriptor mismatch"),
            "err: {err:?}"
        );
    }

    #[test]
    fn verifier_accepts_matching_cdylib_with_imported_agent() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            temp.path().join("math.cor"),
            r#"
public agent inc(x: Int) -> Int:
    return x + 1
"#,
        )
        .expect("write imported module");
        let src = temp.path().join("answer.cor");
        std::fs::write(
            &src,
            r#"
import "./math" as math

pub extern "c"
agent answer(x: Int) -> Int:
    return math.inc(x)
"#,
        )
        .expect("write source");
        let build = build_target_to_disk(&src, BuildTarget::Cdylib, false, false, &[], None)
            .expect("build matching cdylib with import");
        assert!(build.diagnostics.is_empty(), "{:?}", build.diagnostics);
        let cdylib = build.output_path.expect("cdylib path");

        let report = verify_source_matches_cdylib(&src, &cdylib).expect("verify imported source");
        assert!(report.matches(), "report: {report:?}");
    }
}
