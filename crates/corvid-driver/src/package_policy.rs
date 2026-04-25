//! Project package-policy loading and semantic-summary checks.

use std::path::Path;

use anyhow::{anyhow, Result};
use corvid_resolve::ModuleSemanticSummary;
use corvid_types::{CorvidConfig, PackagePolicyConfig};

pub(crate) fn load_package_policy(project_dir: &Path) -> Result<PackagePolicyConfig> {
    let config_path = project_dir.join("corvid.toml");
    let Some(config) = CorvidConfig::load_from_path(&config_path)
        .map_err(|err| anyhow!("failed to load `{}`: {err}", config_path.display()))?
    else {
        return Ok(PackagePolicyConfig::default());
    };
    Ok(config.package_policy)
}

pub(crate) fn package_policy_violation(
    summary: &ModuleSemanticSummary,
    policy: &PackagePolicyConfig,
    signature: Option<&str>,
) -> Option<String> {
    if policy.require_package_signatures && signature.is_none() {
        return Some(
            "package is unsigned, but package-policy.require-package-signatures=true".to_string(),
        );
    }
    if !policy.allow_approval_required {
        if let Some(export) = summary
            .exports
            .values()
            .find(|export| export.approval_required)
        {
            return Some(format!(
                "package export `{}` requires approval, but package-policy.allow-approval-required=false",
                export.name
            ));
        }
    }
    if !policy.allow_effect_violations {
        if let Some(agent) = summary.agents.values().find(|agent| !agent.violations.is_empty()) {
            return Some(format!(
                "package agent `{}` has effect violations, but package-policy.allow-effect-violations=false",
                agent.name
            ));
        }
    }
    if policy.require_deterministic {
        if let Some(agent) = summary.agents.values().find(|agent| !agent.deterministic) {
            return Some(format!(
                "package agent `{}` is not @deterministic, but package-policy.require-deterministic=true",
                agent.name
            ));
        }
    }
    if policy.require_replayable {
        if let Some(agent) = summary.agents.values().find(|agent| !agent.replayable) {
            return Some(format!(
                "package agent `{}` is not @replayable, but package-policy.require-replayable=true",
                agent.name
            ));
        }
    }
    None
}
