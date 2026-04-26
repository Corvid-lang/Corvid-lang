use std::path::{Path, PathBuf};

use anyhow::Result;
use corvid_driver::{inspect_import_semantics, summarize_module_file, NamedModuleSemanticSummary};
use corvid_resolve::{AgentSemanticSummary, DeclKind, ModuleSemanticSummary};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct AuditFinding {
    pub severity: String,
    pub module: String,
    pub target: String,
    pub category: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditModuleReport {
    pub module: String,
    pub path: PathBuf,
    pub exports: usize,
    pub agents: usize,
    pub findings: Vec<AuditFinding>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditReport {
    pub root: PathBuf,
    pub module_count: usize,
    pub finding_count: usize,
    pub findings: Vec<AuditFinding>,
    pub modules: Vec<AuditModuleReport>,
}

pub fn run_audit(path: &Path, json: bool) -> Result<u8> {
    let report = build_audit_report(path)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", render_audit_report(&report));
    }
    Ok(if report.findings.is_empty() { 0 } else { 1 })
}

pub fn build_audit_report(path: &Path) -> Result<AuditReport> {
    let root_summary = summarize_module_file(path)?;
    let mut modules = vec![module_report("root".to_string(), path.to_path_buf(), &root_summary)];
    for imported in inspect_import_semantics(path)? {
        modules.push(imported_module_report(&imported));
    }
    modules.sort_by(|left, right| left.module.cmp(&right.module));
    let findings = modules
        .iter()
        .flat_map(|module| module.findings.clone())
        .collect::<Vec<_>>();
    Ok(AuditReport {
        root: path.to_path_buf(),
        module_count: modules.len(),
        finding_count: findings.len(),
        findings,
        modules,
    })
}

pub fn render_audit_report(report: &AuditReport) -> String {
    let mut out = String::new();
    out.push_str(&format!("Audit report for `{}`\n\n", report.root.display()));
    out.push_str(&format!(
        "Modules: {}  Findings: {}\n\n",
        report.module_count, report.finding_count
    ));
    if report.findings.is_empty() {
        out.push_str("No launch-blocking findings found in the static module contract.\n");
        return out;
    }
    for finding in &report.findings {
        out.push_str(&format!(
            "- [{}] {} :: {} :: {} — {}\n",
            finding.severity, finding.module, finding.target, finding.category, finding.detail
        ));
    }
    out
}

fn imported_module_report(imported: &NamedModuleSemanticSummary) -> AuditModuleReport {
    module_report(
        imported.import.clone(),
        imported.path.clone(),
        &imported.summary,
    )
}

fn module_report(module: String, path: PathBuf, summary: &ModuleSemanticSummary) -> AuditModuleReport {
    let mut findings = Vec::new();
    for export in summary.exports.values() {
        if export.approval_required {
            findings.push(finding(
                "warn",
                &module,
                &export.name,
                "approval-boundary",
                "approval-gated export; verify the approver path is intentional and tested",
            ));
        }
        if !export.replayable && matches!(export.kind, DeclKind::Agent | DeclKind::Tool | DeclKind::Prompt) {
            findings.push(finding(
                "warn",
                &module,
                &export.name,
                "replay-coverage",
                "public AI surface is not marked @replayable or @deterministic",
            ));
        }
        if export.grounded_return && !export.grounded_source {
            findings.push(finding(
                "warn",
                &module,
                &export.name,
                "grounding",
                "grounded return exists without a grounded source export in the same module; review provenance boundaries",
            ));
        }
        if export.effect_names.iter().any(|effect| is_secret_effect(effect)) {
            findings.push(finding(
                "warn",
                &module,
                &export.name,
                "secret-access",
                "effect surface touches secret-bearing capabilities",
            ));
        }
        if export.effect_names.iter().any(|effect| is_money_effect(effect)) {
            findings.push(finding(
                "warn",
                &module,
                &export.name,
                "money-moving-path",
                "effect surface suggests money movement or payment side effects",
            ));
        }
    }
    for agent in summary.agents.values() {
        findings.extend(agent_findings(&module, agent));
    }
    AuditModuleReport {
        module,
        path,
        exports: summary.exports.len(),
        agents: summary.agents.len(),
        findings,
    }
}

fn agent_findings(module: &str, agent: &AgentSemanticSummary) -> Vec<AuditFinding> {
    let mut findings = Vec::new();
    for violation in &agent.violations {
        let category = if violation.contains("budget") || violation.contains("cost") {
            "budget-exposure"
        } else if violation.contains("ungrounded") || violation.contains("grounded") {
            "ungrounded-output"
        } else if violation.contains("policy") || violation.contains("trust") {
            "provider-policy"
        } else {
            "effect-violation"
        };
        findings.push(finding("error", module, &agent.name, category, violation));
    }
    if agent.approval_required {
        findings.push(finding(
            "warn",
            module,
            &agent.name,
            "approval-boundary",
            "agent requires approval; verify launch docs and tests cover the approval path",
        ));
    }
    if let Some(cost) = &agent.cost {
        if is_zero_cost(cost) {
            return findings;
        }
        findings.push(finding(
            "info",
            module,
            &agent.name,
            "budget-exposure",
            &format!("declared worst-case cost dimension: {}", format_dimension_value(cost)),
        ));
    }
    findings
}

fn finding(severity: &str, module: &str, target: &str, category: &str, detail: &str) -> AuditFinding {
    AuditFinding {
        severity: severity.to_string(),
        module: module.to_string(),
        target: target.to_string(),
        category: category.to_string(),
        detail: detail.to_string(),
    }
}

fn is_secret_effect(effect: &str) -> bool {
    let effect = effect.to_ascii_lowercase();
    effect.contains("secret")
}

fn is_money_effect(effect: &str) -> bool {
    let effect = effect.to_ascii_lowercase();
    ["pay", "paid", "refund", "money", "billing", "charge", "settle"]
        .iter()
        .any(|needle| effect.contains(needle))
}

fn is_zero_cost(value: &corvid_ast::DimensionValue) -> bool {
    matches!(value, corvid_ast::DimensionValue::Cost(v) if *v == 0.0)
}

fn format_dimension_value(value: &corvid_ast::DimensionValue) -> String {
    match value {
        corvid_ast::DimensionValue::Bool(v) => v.to_string(),
        corvid_ast::DimensionValue::Name(v) => v.clone(),
        corvid_ast::DimensionValue::Cost(v) => format!("${v:.6}"),
        corvid_ast::DimensionValue::Number(v) => format!("{v:.3}"),
        corvid_ast::DimensionValue::Streaming { backpressure } => backpressure.label(),
        corvid_ast::DimensionValue::ConfidenceGated {
            threshold,
            above,
            below,
        } => format!("{}_if_confident({threshold:.3}) else {}", above, below),
    }
}
