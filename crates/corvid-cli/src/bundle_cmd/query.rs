use anyhow::{bail, Context, Result};
use corvid_abi::{read_descriptor_from_path, CorvidAbi};
use corvid_driver::build_catalog_descriptor_for_source;
use serde::Serialize;
use std::fs;
use std::path::Path;

use crate::trace_diff::{
    apply_splices_for_bundle_query, compute_bundle_diff_summary, compute_splices_for_bundle_delta,
    prepare_bundle_isolation_input,
};

use super::manifest::LoadedManifest;

pub fn run_query(
    bundle: &Path,
    delta: &str,
    predecessor: Option<&str>,
    json: bool,
) -> Result<u8> {
    let loaded = LoadedManifest::load(bundle)?;
    let predecessor_path = loaded.predecessor_path(predecessor)?;
    let predecessor_bundle = LoadedManifest::load(&predecessor_path)?;

    let parent_source = fs::read_to_string(predecessor_bundle.primary_source_path()).with_context(|| {
        format!(
            "read predecessor source `{}`",
            predecessor_bundle.primary_source_path().display()
        )
    })?;
    let commit_source = fs::read_to_string(loaded.primary_source_path())
        .with_context(|| format!("read bundle source `{}`", loaded.primary_source_path().display()))?;

    let parent_input = prepare_bundle_isolation_input(&parent_source)
        .map_err(|err| anyhow::anyhow!("BundleCounterfactualUnsupported: {err}"))?;
    let commit_input = prepare_bundle_isolation_input(&commit_source)
        .map_err(|err| anyhow::anyhow!("BundleCounterfactualUnsupported: {err}"))?;
    let splices = compute_splices_for_bundle_delta(delta, &parent_input, &commit_input)
        .map_err(|err| anyhow::anyhow!("BundleCounterfactualUnsupported: {err}"))?;
    let synthesized_source = apply_splices_for_bundle_query(&parent_source, splices)
        .map_err(|err| anyhow::anyhow!("BundleCounterfactualUnsupported: {err}"))?;

    let temp = tempfile::tempdir().context("create bundle query tempdir")?;
    let temp_source = temp.path().join(
        loaded
            .primary_source_path()
            .file_name()
            .unwrap_or(std::ffi::OsStr::new("counterfactual.cor")),
    );
    fs::write(&temp_source, synthesized_source).context("write counterfactual source")?;

    let parent_abi = read_descriptor_from_path(&predecessor_bundle.descriptor_path())?;
    let current_abi = read_descriptor_from_path(&loaded.descriptor_path())?;
    let counterfactual_abi = build_counterfactual_abi(&temp_source)?;

    let isolated = compute_bundle_diff_summary(&parent_abi, &counterfactual_abi);
    if !isolated.records.iter().any(|record| record.key == delta) {
        bail!(
            "BundleCounterfactualDeltaMismatch: synthesized attestation diff did not include `{}`; got {:?}",
            delta,
            isolated.records.iter().map(|record| record.key.as_str()).collect::<Vec<_>>()
        );
    }
    let current = compute_bundle_diff_summary(&parent_abi, &current_abi);

    let report = BundleQueryReport {
        bundle: loaded.manifest.name.clone(),
        predecessor: predecessor_bundle.manifest.name.clone(),
        delta: delta.to_string(),
        isolated_attestation_diff: isolated
            .records
            .into_iter()
            .map(|record| DiffRecord {
                key: record.key,
                summary: record.summary,
            })
            .collect(),
        current_attestation_diff: current
            .records
            .into_iter()
            .map(|record| DiffRecord {
                key: record.key,
                summary: record.summary,
            })
            .collect(),
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("bundle query: {}", report.bundle);
        println!("predecessor: {}", report.predecessor);
        println!("delta: {}", report.delta);
        println!("isolated attestation diff:");
        for record in &report.isolated_attestation_diff {
            println!("  - {} :: {}", record.key, record.summary);
        }
        println!("current attestation diff:");
        for record in &report.current_attestation_diff {
            println!("  - {} :: {}", record.key, record.summary);
        }
    }

    Ok(0)
}

fn build_counterfactual_abi(source_path: &Path) -> Result<CorvidAbi> {
    let output = build_catalog_descriptor_for_source(source_path)
        .with_context(|| format!("build counterfactual descriptor from `{}`", source_path.display()))?;
    if !output.diagnostics.is_empty() {
        bail!(
            "BundleCounterfactualCompileFailed: counterfactual source surfaced {} diagnostic(s)",
            output.diagnostics.len()
        );
    }
    let json = output
        .descriptor_json
        .ok_or_else(|| anyhow::anyhow!("BundleCounterfactualCompileFailed: descriptor rebuild produced no JSON"))?;
    serde_json::from_str(&json).context("parse counterfactual descriptor JSON")
}

#[derive(Debug, Serialize)]
struct BundleQueryReport {
    bundle: String,
    predecessor: String,
    delta: String,
    isolated_attestation_diff: Vec<DiffRecord>,
    current_attestation_diff: Vec<DiffRecord>,
}

#[derive(Debug, Serialize)]
struct DiffRecord {
    key: String,
    summary: String,
}
