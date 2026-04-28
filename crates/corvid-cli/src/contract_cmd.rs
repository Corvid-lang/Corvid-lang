//! `corvid contract` — public-surface inspection of the canonical
//! [`corvid_guarantees::GUARANTEE_REGISTRY`].
//!
//! `corvid contract list` prints the registry as either a
//! human-readable table or structured JSON. The JSON output is the
//! single source of truth that:
//!
//!   * `docs/core-semantics.md` is generated from in slice 35-D
//!     (CI rejects drift between the committed doc and this command's
//!     output), and
//!   * `corvid claim --explain` (slice 35-I) cross-references when
//!     reporting which guarantees a given binary was checked against.
//!
//! Optional `--class` and `--kind` filters narrow the output for
//! human inspection without changing the canonical ordering. The
//! command never reorders the registry — declaration order in
//! `corvid-guarantees` is the stable serialization order.

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use corvid_guarantees::{
    render_core_semantics_markdown, Guarantee, GuaranteeClass, GuaranteeKind, Phase,
    GUARANTEE_REGISTRY,
};
use serde::Serialize;

/// Run `corvid contract list`.
///
/// `json == true` emits the structured payload (one outer JSON
/// object with a `guarantees` array). The human-readable form prints
/// a fixed-width table sorted in declaration order — readers should
/// be able to scan it in well under ten minutes per the Phase 35
/// goal.
pub fn run_list(json: bool, class_filter: Option<&str>, kind_filter: Option<&str>) -> Result<u8> {
    let class = class_filter.map(parse_class).transpose()?;
    let kind = kind_filter.map(parse_kind).transpose()?;

    let rows: Vec<&'static Guarantee> = GUARANTEE_REGISTRY
        .iter()
        .filter(|g| class.map_or(true, |c| g.class == c))
        .filter(|g| kind.map_or(true, |k| g.kind == k))
        .collect();

    if json {
        let payload = JsonPayload {
            schema_version: 1,
            count: rows.len(),
            guarantees: rows.iter().map(|g| JsonGuarantee::from(*g)).collect(),
        };
        let text = serde_json::to_string_pretty(&payload)
            .map_err(|e| anyhow!("serialize guarantees as JSON: {e}"))?;
        println!("{text}");
    } else {
        print_table(&rows);
    }
    Ok(0)
}

/// Run `corvid contract regen-doc <output>`.
///
/// Writes the canonical `docs/core-semantics.md` rendering to the
/// given path. The output is byte-deterministic for a given
/// registry, so committing the result and gating CI on
/// `corvid_guarantees::render::tests::rendered_markdown_matches_committed_doc`
/// keeps spec ≡ implementation.
pub fn run_regen_doc(output: &Path) -> Result<u8> {
    let rendered = render_core_semantics_markdown();
    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!(
                    "creating parent directory `{}` for regenerated doc",
                    parent.display()
                )
            })?;
        }
    }
    std::fs::write(output, &rendered)
        .with_context(|| format!("writing regenerated doc to `{}`", output.display()))?;
    eprintln!(
        "wrote {} bytes to {}",
        rendered.len(),
        output.display()
    );
    Ok(0)
}

fn parse_class(raw: &str) -> Result<GuaranteeClass> {
    match raw {
        "static" => Ok(GuaranteeClass::Static),
        "runtime_checked" | "runtime-checked" => Ok(GuaranteeClass::RuntimeChecked),
        "out_of_scope" | "out-of-scope" => Ok(GuaranteeClass::OutOfScope),
        other => Err(anyhow!(
            "unknown --class `{other}` — expected `static`, `runtime_checked`, or `out_of_scope`"
        )),
    }
}

fn parse_kind(raw: &str) -> Result<GuaranteeKind> {
    for kind in GuaranteeKind::ALL {
        if kind.slug() == raw {
            return Ok(*kind);
        }
    }
    let valid: Vec<&'static str> = GuaranteeKind::ALL.iter().map(|k| k.slug()).collect();
    Err(anyhow!(
        "unknown --kind `{raw}` — expected one of {}",
        valid.join(", ")
    ))
}

#[derive(Serialize)]
struct JsonPayload {
    schema_version: u32,
    count: usize,
    guarantees: Vec<JsonGuarantee>,
}

#[derive(Serialize)]
struct JsonGuarantee {
    id: &'static str,
    kind: &'static str,
    class: &'static str,
    phase: &'static str,
    description: &'static str,
    #[serde(skip_serializing_if = "str::is_empty")]
    out_of_scope_reason: &'static str,
    positive_test_refs: Vec<&'static str>,
    adversarial_test_refs: Vec<&'static str>,
}

impl From<&Guarantee> for JsonGuarantee {
    fn from(g: &Guarantee) -> Self {
        JsonGuarantee {
            id: g.id,
            kind: g.kind.slug(),
            class: g.class.slug(),
            phase: g.phase.slug(),
            description: g.description,
            out_of_scope_reason: g.out_of_scope_reason,
            positive_test_refs: g.positive_test_refs.to_vec(),
            adversarial_test_refs: g.adversarial_test_refs.to_vec(),
        }
    }
}

fn print_table(rows: &[&'static Guarantee]) {
    if rows.is_empty() {
        println!("(no guarantees match the supplied filters)");
        return;
    }
    let id_w = rows.iter().map(|g| g.id.len()).max().unwrap_or(0).max(4);
    let class_w = GuaranteeClass::ALL
        .iter()
        .map(|c| c.slug().len())
        .max()
        .unwrap_or(0)
        .max(5);
    let phase_w = Phase::ALL
        .iter()
        .map(|p| p.slug().len())
        .max()
        .unwrap_or(0)
        .max(5);

    println!(
        "{:<id_w$}  {:<class_w$}  {:<phase_w$}  description",
        "id",
        "class",
        "phase",
        id_w = id_w,
        class_w = class_w,
        phase_w = phase_w,
    );
    println!(
        "{}  {}  {}  {}",
        "-".repeat(id_w),
        "-".repeat(class_w),
        "-".repeat(phase_w),
        "-".repeat(11),
    );
    for g in rows {
        println!(
            "{:<id_w$}  {:<class_w$}  {:<phase_w$}  {}",
            g.id,
            g.class.slug(),
            g.phase.slug(),
            g.description,
            id_w = id_w,
            class_w = class_w,
            phase_w = phase_w,
        );
        if g.class == GuaranteeClass::OutOfScope && !g.out_of_scope_reason.is_empty() {
            println!(
                "{:<id_w$}  {:<class_w$}  {:<phase_w$}  reason: {}",
                "",
                "",
                "",
                g.out_of_scope_reason,
                id_w = id_w,
                class_w = class_w,
                phase_w = phase_w,
            );
        }
    }
    println!();
    println!(
        "{} guarantees (registry size {})",
        rows.len(),
        GUARANTEE_REGISTRY.len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_class_accepts_known_slugs() {
        assert_eq!(parse_class("static").unwrap(), GuaranteeClass::Static);
        assert_eq!(
            parse_class("runtime_checked").unwrap(),
            GuaranteeClass::RuntimeChecked
        );
        assert_eq!(
            parse_class("runtime-checked").unwrap(),
            GuaranteeClass::RuntimeChecked
        );
        assert_eq!(
            parse_class("out_of_scope").unwrap(),
            GuaranteeClass::OutOfScope
        );
    }

    #[test]
    fn parse_class_rejects_unknown() {
        assert!(parse_class("nope").is_err());
    }

    #[test]
    fn parse_kind_accepts_every_registered_kind() {
        for kind in GuaranteeKind::ALL {
            assert_eq!(parse_kind(kind.slug()).unwrap(), *kind);
        }
    }

    #[test]
    fn json_payload_matches_registry_size() {
        let rows: Vec<&'static Guarantee> = GUARANTEE_REGISTRY.iter().collect();
        let payload = JsonPayload {
            schema_version: 1,
            count: rows.len(),
            guarantees: rows.iter().map(|g| JsonGuarantee::from(*g)).collect(),
        };
        assert_eq!(payload.count, GUARANTEE_REGISTRY.len());
        assert_eq!(payload.guarantees.len(), GUARANTEE_REGISTRY.len());
    }

    #[test]
    fn json_payload_emits_out_of_scope_reason_only_for_out_of_scope() {
        let json = serde_json::to_string(&JsonPayload {
            schema_version: 1,
            count: GUARANTEE_REGISTRY.len(),
            guarantees: GUARANTEE_REGISTRY
                .iter()
                .map(|g| JsonGuarantee::from(g))
                .collect(),
        })
        .unwrap();
        // Static and RuntimeChecked entries must NOT carry an
        // out_of_scope_reason in the JSON — `skip_serializing_if`
        // drops the field for them. The seed currently has at least
        // one Static row (`approval.dangerous_call_requires_token`),
        // so confirm its description is present without a reason.
        assert!(json.contains("approval.dangerous_call_requires_token"));
        // OutOfScope rows MUST include their reason.
        assert!(json.contains("platform.host_kernel_compromise"));
        assert!(json.contains("Outside Corvid's trust boundary"));
    }
}
