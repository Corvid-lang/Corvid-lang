use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

use crate::render::{render_data, render_latency, render_trust};
use crate::{
    BlameAttribution, Divergence, DivergenceClass, EffectProfile, LatencyLevel, Tier, TierReport,
    TrustLevel,
};

pub(crate) fn diff_reports(reports: &[TierReport; 4]) -> Result<Vec<Divergence>> {
    let mut divergences = Vec::new();

    maybe_push_divergence(
        &mut divergences,
        "cost",
        value_map(reports, |profile| serde_json::json!(profile.cost)),
        reports
            .iter()
            .all(|report| report.profile.cost == reports[0].profile.cost),
        reports,
    )?;
    maybe_push_divergence(
        &mut divergences,
        "trust",
        value_map(reports, |profile| {
            serde_json::json!(render_trust(&profile.trust))
        }),
        reports
            .iter()
            .all(|report| report.profile.trust == reports[0].profile.trust),
        reports,
    )?;
    maybe_push_divergence(
        &mut divergences,
        "reversible",
        value_map(reports, |profile| serde_json::json!(profile.reversible)),
        reports
            .iter()
            .all(|report| report.profile.reversible == reports[0].profile.reversible),
        reports,
    )?;
    maybe_push_divergence(
        &mut divergences,
        "data",
        value_map(reports, |profile| {
            serde_json::json!(render_data(&profile.data))
        }),
        reports
            .iter()
            .all(|report| report.profile.data == reports[0].profile.data),
        reports,
    )?;
    maybe_push_divergence(
        &mut divergences,
        "latency",
        value_map(reports, |profile| {
            serde_json::json!(render_latency(&profile.latency))
        }),
        reports
            .iter()
            .all(|report| report.profile.latency == reports[0].profile.latency),
        reports,
    )?;
    maybe_push_divergence(
        &mut divergences,
        "confidence",
        value_map(reports, |profile| serde_json::json!(profile.confidence)),
        reports
            .iter()
            .all(|report| report.profile.confidence == reports[0].profile.confidence),
        reports,
    )?;

    let extra_keys: BTreeSet<_> = reports
        .iter()
        .flat_map(|report| report.profile.extra.keys().cloned())
        .collect();
    for key in extra_keys {
        let values = value_map(reports, |profile| {
            serde_json::to_value(profile.extra.get(&key)).unwrap_or(serde_json::Value::Null)
        });
        let all_equal = values
            .values()
            .all(|value| value == values.get(&Tier::Checker).unwrap());
        if !all_equal {
            divergences.push(Divergence {
                dimension: key.clone(),
                classification: DivergenceClass::TierMismatch,
                values,
                attribution: divergent_tier_blame("extra")?,
            });
        }
    }

    Ok(divergences)
}

fn maybe_push_divergence(
    divergences: &mut Vec<Divergence>,
    dimension: &str,
    values: BTreeMap<Tier, serde_json::Value>,
    all_equal: bool,
    reports: &[TierReport; 4],
) -> Result<()> {
    if all_equal {
        return Ok(());
    }
    let classification = if has_tier_overapproximation(dimension, reports) {
        DivergenceClass::StaticOverapproximated
    } else if has_tier_too_loose(dimension, reports) {
        DivergenceClass::StaticTooLoose
    } else {
        DivergenceClass::TierMismatch
    };
    divergences.push(Divergence {
        dimension: dimension.into(),
        classification,
        values,
        attribution: divergent_tier_blame(dimension)?,
    });
    Ok(())
}

fn has_tier_overapproximation(dimension: &str, reports: &[TierReport; 4]) -> bool {
    let interpreter = &reports[1].profile;
    reports
        .iter()
        .enumerate()
        .filter(|(idx, _)| *idx != 1)
        .any(|(_, report)| is_profile_overapproximation(dimension, &report.profile, interpreter))
}

fn has_tier_too_loose(dimension: &str, reports: &[TierReport; 4]) -> bool {
    let interpreter = &reports[1].profile;
    reports
        .iter()
        .enumerate()
        .filter(|(idx, _)| *idx != 1)
        .any(|(_, report)| is_profile_too_loose(dimension, &report.profile, interpreter))
}

fn value_map(
    reports: &[TierReport; 4],
    value_of: impl Fn(&EffectProfile) -> serde_json::Value,
) -> BTreeMap<Tier, serde_json::Value> {
    reports
        .iter()
        .map(|report| (report.tier, value_of(&report.profile)))
        .collect()
}

fn is_profile_overapproximation(
    dimension: &str,
    candidate: &EffectProfile,
    baseline: &EffectProfile,
) -> bool {
    match dimension {
        "cost" => candidate.cost > baseline.cost,
        "trust" => trust_rank(&candidate.trust) > trust_rank(&baseline.trust),
        "reversible" => !candidate.reversible && baseline.reversible,
        "data" => candidate.data.is_superset(&baseline.data) && candidate.data != baseline.data,
        "latency" => latency_rank(&candidate.latency) > latency_rank(&baseline.latency),
        "confidence" => candidate.confidence < baseline.confidence,
        _ => false,
    }
}

fn is_profile_too_loose(
    dimension: &str,
    candidate: &EffectProfile,
    baseline: &EffectProfile,
) -> bool {
    match dimension {
        "cost" => candidate.cost < baseline.cost,
        "trust" => trust_rank(&candidate.trust) < trust_rank(&baseline.trust),
        "reversible" => candidate.reversible && !baseline.reversible,
        "data" => !candidate.data.is_superset(&baseline.data),
        "latency" => latency_rank(&candidate.latency) < latency_rank(&baseline.latency),
        "confidence" => candidate.confidence > baseline.confidence,
        _ => false,
    }
}

fn divergent_tier_blame(dimension: &str) -> Result<Vec<BlameAttribution>> {
    let targets = match dimension {
        "cost" | "trust" | "reversible" | "data" | "latency" | "confidence" => vec![
            (
                Tier::Checker,
                PathBuf::from("crates/corvid-types/src/effects.rs"),
            ),
            (
                Tier::Interpreter,
                PathBuf::from("crates/corvid-vm/src/interp.rs"),
            ),
            (
                Tier::Native,
                PathBuf::from("crates/corvid-runtime/src/ffi_bridge.rs"),
            ),
            (
                Tier::Replay,
                PathBuf::from("crates/corvid-runtime/src/tracing.rs"),
            ),
        ],
        _ => vec![(
            Tier::Checker,
            PathBuf::from("crates/corvid-types/src/effects.rs"),
        )],
    };

    let mut attributions = Vec::new();
    for (tier, file) in targets {
        if let Some((commit, author)) = blame_file(&file)? {
            attributions.push(BlameAttribution {
                tier,
                file,
                commit,
                author,
            });
        }
    }
    Ok(attributions)
}

fn blame_file(path: &Path) -> Result<Option<(String, String)>> {
    let output = Command::new("git")
        .arg("blame")
        .arg("-L")
        .arg("1,1")
        .arg("--porcelain")
        .arg(path)
        .output()
        .with_context(|| format!("failed to run git blame for `{}`", path.display()))?;
    if !output.status.success() {
        return Ok(None);
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut commit = String::new();
    let mut author = String::new();
    for (index, line) in stdout.lines().enumerate() {
        if index == 0 {
            commit = line.split_whitespace().next().unwrap_or("").to_string();
        } else if let Some(rest) = line.strip_prefix("author ") {
            author = rest.to_string();
            break;
        }
    }
    if commit.is_empty() || author.is_empty() {
        Ok(None)
    } else {
        Ok(Some((commit, author)))
    }
}

fn trust_rank(level: &TrustLevel) -> u8 {
    match level {
        TrustLevel::Autonomous => 0,
        TrustLevel::SupervisorRequired => 1,
        TrustLevel::HumanRequired => 2,
        TrustLevel::Custom(_) => 3,
    }
}

fn latency_rank(level: &LatencyLevel) -> u8 {
    match level {
        LatencyLevel::Instant => 0,
        LatencyLevel::Fast => 1,
        LatencyLevel::Medium => 2,
        LatencyLevel::Slow => 3,
        LatencyLevel::Streaming { .. } => 4,
        LatencyLevel::Custom(_) => 5,
    }
}
