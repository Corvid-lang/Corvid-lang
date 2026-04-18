//! `corvid add-dimension` — install a dimension into `corvid.toml`.
//!
//! Two addressing forms:
//!
//!   * **Local path**: `corvid add-dimension ./freshness.dim.toml`
//!     Reads a TOML file whose shape matches the `[effect-system.dimensions.*]`
//!     section of `corvid.toml`, validates it, then appends the
//!     declaration to the project's `corvid.toml`. The local form
//!     is the MVP — it doesn't require any registry infrastructure.
//!
//!   * **Registry**: `corvid add-dimension fairness@1.0`
//!     Not yet implemented — the Corvid effect registry isn't hosted
//!     yet. Surfaces a clear error pointing users at the local form
//!     and tracking this as follow-up work in ROADMAP Phase 20g #9.
//!
//! Validation mirrors `CorvidConfig::into_dimension_schemas`:
//!   * composition must be one of the five archetypes
//!   * type must be one of the six value kinds
//!   * default must parse against the declared type
//!   * name must not collide with a built-in
//!   * name must not already exist in the project's `corvid.toml`
//!
//! After validation, the dimension's claimed archetype laws are
//! verified via the `corvid-types::law_check` harness so callers
//! catch algebra-violating declarations at install time, not at
//! first compile.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use corvid_types::{check_dimension, CorvidConfig, DimensionUnderTest};

/// Outcome of `add_dimension`. `Added` is the happy path; `Rejected`
/// carries a human-readable reason when validation fails.
#[derive(Debug, Clone)]
pub enum AddDimensionOutcome {
    Added {
        name: String,
        target: PathBuf,
    },
    Rejected {
        reason: String,
    },
}

/// Parse the spec string and install the referenced dimension.
///
/// `spec` is either:
///   * `name@version` — resolved against the registry (not yet
///     implemented — returns an error with clear remediation)
///   * any other string — treated as a filesystem path to a TOML
///     fragment matching the `[effect-system.dimensions.*]` shape
///
/// `project_dir` is the directory that contains (or will contain)
/// `corvid.toml`. If no file exists, one is created.
pub fn add_dimension(spec: &str, project_dir: &Path) -> Result<AddDimensionOutcome> {
    if is_registry_form(spec) {
        return Ok(AddDimensionOutcome::Rejected {
            reason: format!(
                "registry form `{spec}` is not yet implemented — the Corvid effect \
                 registry isn't hosted yet. Use the local form instead: save the \
                 dimension declaration to a local `.toml` file and run \
                 `corvid add-dimension ./that-file.toml`. Tracked in ROADMAP \
                 Phase 20g #9."
            ),
        });
    }

    let source_path = PathBuf::from(spec);
    install_from_path(&source_path, project_dir)
}

fn is_registry_form(spec: &str) -> bool {
    // A registry spec contains `@` with text on both sides and is NOT
    // a path (no `/` or `\`, doesn't end in `.toml`). Anything with a
    // path separator is a local file.
    if spec.contains('/') || spec.contains('\\') {
        return false;
    }
    if spec.ends_with(".toml") {
        return false;
    }
    if let Some((name, version)) = spec.split_once('@') {
        return !name.is_empty() && !version.is_empty();
    }
    false
}

fn install_from_path(source: &Path, project_dir: &Path) -> Result<AddDimensionOutcome> {
    let bytes = fs::read_to_string(source)
        .with_context(|| format!("cannot read dimension file `{}`", source.display()))?;
    let fragment: CorvidConfig = toml::from_str(&bytes)
        .with_context(|| format!("failed to parse `{}` as TOML", source.display()))?;

    let schemas = fragment
        .into_dimension_schemas()
        .map_err(|e| anyhow!("dimension validation failed: {e}"))?;

    if schemas.is_empty() {
        return Ok(AddDimensionOutcome::Rejected {
            reason: format!(
                "`{}` has no `[effect-system.dimensions.*]` entries — nothing to install",
                source.display()
            ),
        });
    }
    if schemas.len() > 1 {
        return Ok(AddDimensionOutcome::Rejected {
            reason: format!(
                "`{}` declares {} dimensions; install them one at a time so each \
                 passes its law check independently",
                source.display(),
                schemas.len()
            ),
        });
    }

    let (schema, meta) = schemas.into_iter().next().unwrap();
    let dim_name = schema.name.clone();

    // Reject if the project's existing corvid.toml already declares
    // this dimension — overwriting would silently change semantics.
    let target = project_dir.join("corvid.toml");
    if let Some(existing) = CorvidConfig::load_from_path(&target)
        .map_err(|e| anyhow!("failed to parse existing corvid.toml: {e}"))?
    {
        if existing.effect_system.dimensions.contains_key(&dim_name) {
            return Ok(AddDimensionOutcome::Rejected {
                reason: format!(
                    "dimension `{dim_name}` already exists in `{}`; remove it from \
                     the file first or pick a different name",
                    target.display()
                ),
            });
        }
    }

    // Run the law-check harness on the incoming dimension before
    // writing anything — catches declarations whose archetype + type
    // + default combination violates algebraic laws.
    let under_test = DimensionUnderTest::from_custom(schema.clone(), &meta);
    let results = check_dimension(&under_test, 5_000);
    let mut law_failures: Vec<String> = Vec::new();
    for r in &results {
        if let corvid_types::Verdict::CounterExample { note, .. } = &r.verdict {
            law_failures.push(format!("{} — {note}", r.law.as_str()));
        }
    }
    if !law_failures.is_empty() {
        return Ok(AddDimensionOutcome::Rejected {
            reason: format!(
                "dimension `{dim_name}` violates its archetype's laws; install refused. \
                 Failures:\n    {}",
                law_failures.join("\n    ")
            ),
        });
    }

    // Serialize the new section and append to corvid.toml. If the
    // file doesn't exist, create it with only the new section.
    let section = render_dimension_section(&dim_name, &meta, &schema);
    let next_contents = match fs::read_to_string(&target) {
        Ok(existing) => append_section(&existing, &section),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => section.clone(),
        Err(e) => {
            return Err(anyhow!(
                "failed to read target `{}`: {e}",
                target.display()
            ))
        }
    };
    fs::create_dir_all(project_dir).with_context(|| {
        format!("failed to create `{}`", project_dir.display())
    })?;
    fs::write(&target, next_contents).with_context(|| {
        format!("failed to write `{}`", target.display())
    })?;

    Ok(AddDimensionOutcome::Added {
        name: dim_name,
        target,
    })
}

fn append_section(existing: &str, section: &str) -> String {
    let mut out = existing.to_string();
    if !out.ends_with('\n') {
        out.push('\n');
    }
    // Visual break between whatever was there and the new section.
    if !out.trim_end().is_empty() {
        out.push('\n');
    }
    out.push_str(section);
    out
}

fn render_dimension_section(
    name: &str,
    meta: &corvid_types::CustomDimensionMeta,
    schema: &corvid_ast::DimensionSchema,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("[effect-system.dimensions.{name}]\n"));
    out.push_str(&format!(
        "composition = \"{}\"\n",
        composition_name(schema.composition)
    ));
    out.push_str(&format!("type = \"{}\"\n", meta.ty.as_str()));
    out.push_str(&format!(
        "default = \"{}\"\n",
        format_default(&schema.default)
    ));
    if let Some(sem) = &meta.semantics {
        let escaped = sem.replace('"', "\\\"");
        out.push_str(&format!("semantics = \"{escaped}\"\n"));
    }
    if let Some(proof) = &meta.proof_path {
        let escaped = proof.replace('"', "\\\"");
        out.push_str(&format!("proof = \"{escaped}\"\n"));
    }
    out
}

fn composition_name(rule: corvid_ast::CompositionRule) -> &'static str {
    match rule {
        corvid_ast::CompositionRule::Sum => "Sum",
        corvid_ast::CompositionRule::Max => "Max",
        corvid_ast::CompositionRule::Min => "Min",
        corvid_ast::CompositionRule::Union => "Union",
        corvid_ast::CompositionRule::LeastReversible => "LeastReversible",
    }
}

fn format_default(value: &corvid_ast::DimensionValue) -> String {
    match value {
        corvid_ast::DimensionValue::Bool(b) => b.to_string(),
        corvid_ast::DimensionValue::Name(n) => n.clone(),
        corvid_ast::DimensionValue::Cost(c) => format!("{c}"),
        corvid_ast::DimensionValue::Number(n) => {
            if n.is_infinite() {
                if n.is_sign_positive() {
                    "inf".into()
                } else {
                    "-inf".into()
                }
            } else {
                format!("{n}")
            }
        }
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(root: &Path, name: &str, body: &str) -> PathBuf {
        let path = root.join(name);
        fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn registry_form_is_surfaced_as_rejected_with_clear_message() {
        let tmp = TempDir::new().unwrap();
        let outcome = add_dimension("fairness@1.0", tmp.path()).unwrap();
        match outcome {
            AddDimensionOutcome::Rejected { reason } => {
                assert!(reason.contains("registry"), "{reason}");
                assert!(reason.contains("local form"), "{reason}");
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn installs_well_formed_dimension_into_new_corvid_toml() {
        let tmp = TempDir::new().unwrap();
        let source = write(
            tmp.path(),
            "freshness.dim.toml",
            r#"
[effect-system.dimensions.freshness]
composition = "Max"
type = "number"
default = "0"
semantics = "max age of data in a call chain"
"#,
        );
        let project = tmp.path().join("project");
        let outcome = add_dimension(source.to_str().unwrap(), &project).unwrap();
        match outcome {
            AddDimensionOutcome::Added { name, target } => {
                assert_eq!(name, "freshness");
                assert!(target.ends_with("corvid.toml"));
                let contents = fs::read_to_string(&target).unwrap();
                assert!(contents.contains("[effect-system.dimensions.freshness]"));
                assert!(contents.contains("composition = \"Max\""));
                assert!(contents.contains("semantics"));
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn appends_to_existing_corvid_toml_preserving_other_content() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        fs::write(
            project.join("corvid.toml"),
            "name = \"demo\"\nversion = \"0.1.0\"\n\n[llm]\ndefault_model = \"claude\"\n",
        )
        .unwrap();
        let source = write(
            tmp.path(),
            "carbon.dim.toml",
            r#"
[effect-system.dimensions.carbon]
composition = "Sum"
type = "number"
default = "0"
"#,
        );
        let outcome = add_dimension(source.to_str().unwrap(), &project).unwrap();
        assert!(matches!(outcome, AddDimensionOutcome::Added { .. }));
        let contents = fs::read_to_string(project.join("corvid.toml")).unwrap();
        // Original content survives.
        assert!(contents.contains("name = \"demo\""));
        assert!(contents.contains("[llm]"));
        // New section appended.
        assert!(contents.contains("[effect-system.dimensions.carbon]"));
    }

    #[test]
    fn rejects_collision_with_existing_dimension_in_project() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        fs::write(
            project.join("corvid.toml"),
            r#"
[effect-system.dimensions.freshness]
composition = "Max"
type = "number"
default = "0"
"#,
        )
        .unwrap();
        let source = write(
            tmp.path(),
            "freshness2.toml",
            r#"
[effect-system.dimensions.freshness]
composition = "Max"
type = "number"
default = "0"
"#,
        );
        let outcome = add_dimension(source.to_str().unwrap(), &project).unwrap();
        match outcome {
            AddDimensionOutcome::Rejected { reason } => {
                assert!(reason.contains("already exists"), "{reason}");
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn rejects_collision_with_builtin_dimension_name() {
        let tmp = TempDir::new().unwrap();
        let source = write(
            tmp.path(),
            "cost.dim.toml",
            r#"
[effect-system.dimensions.cost]
composition = "Sum"
type = "cost"
"#,
        );
        let result = add_dimension(source.to_str().unwrap(), tmp.path());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("built-in"), "{msg}");
    }

    #[test]
    fn rejects_dimension_that_violates_its_archetype_laws() {
        let tmp = TempDir::new().unwrap();
        // Sum archetype with non-zero default breaks identity — the
        // law-check harness catches this at install time so the
        // mis-declaration never reaches the project's corvid.toml.
        let source = write(
            tmp.path(),
            "broken.dim.toml",
            r#"
[effect-system.dimensions.broken_sum]
composition = "Sum"
type = "number"
default = "5"
"#,
        );
        let outcome = add_dimension(source.to_str().unwrap(), tmp.path()).unwrap();
        match outcome {
            AddDimensionOutcome::Rejected { reason } => {
                assert!(reason.contains("laws"), "{reason}");
                assert!(reason.contains("identity"), "{reason}");
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
        // corvid.toml must NOT have been touched.
        assert!(!tmp.path().join("corvid.toml").exists());
    }

    #[test]
    fn rejects_multi_dimension_fragment_with_actionable_message() {
        let tmp = TempDir::new().unwrap();
        let source = write(
            tmp.path(),
            "two.dim.toml",
            r#"
[effect-system.dimensions.a]
composition = "Max"
type = "number"

[effect-system.dimensions.b]
composition = "Max"
type = "number"
"#,
        );
        let outcome = add_dimension(source.to_str().unwrap(), tmp.path()).unwrap();
        match outcome {
            AddDimensionOutcome::Rejected { reason } => {
                assert!(reason.contains("one at a time"), "{reason}");
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn rejects_fragment_with_no_dimensions() {
        let tmp = TempDir::new().unwrap();
        let source = write(
            tmp.path(),
            "empty.toml",
            "name = \"not-a-dimension\"\n",
        );
        let outcome = add_dimension(source.to_str().unwrap(), tmp.path()).unwrap();
        match outcome {
            AddDimensionOutcome::Rejected { reason } => {
                assert!(reason.contains("no"), "{reason}");
                assert!(reason.contains("nothing to install"), "{reason}");
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }
}
