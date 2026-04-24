use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::attest::verify_dsse_envelope;
use super::manifest::LoadedManifest;

pub fn run_lineage(bundle: &Path, json: bool) -> Result<u8> {
    let root = LoadedManifest::load(bundle)?;
    let mut visited = BTreeSet::new();
    let mut nodes = Vec::new();
    walk_bundle(&root, &mut visited, &mut nodes)?;

    let report = LineageReport {
        root_bundle: root.manifest.name.clone(),
        nodes,
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("bundle lineage: {}", report.root_bundle);
        for node in &report.nodes {
            let status = if node.signature_verified { "signed" } else { "unsigned" };
            let indent = "  ".repeat(node.depth);
            println!(
                "{}- {} [{}] {}",
                indent, node.name, status, node.path
            );
        }
    }
    Ok(0)
}

fn walk_bundle(
    loaded: &LoadedManifest,
    visited: &mut BTreeSet<PathBuf>,
    nodes: &mut Vec<LineageNode>,
) -> Result<()> {
    walk_bundle_inner(loaded, visited, nodes, 0)
}

fn walk_bundle_inner(
    loaded: &LoadedManifest,
    visited: &mut BTreeSet<PathBuf>,
    nodes: &mut Vec<LineageNode>,
    depth: usize,
) -> Result<()> {
    let canonical = std::fs::canonicalize(&loaded.manifest_path)
        .with_context(|| format!("canonicalize `{}`", loaded.manifest_path.display()))?;
    if !visited.insert(canonical.clone()) {
        bail!(
            "BundleLineageCycle: bundle `{}` was visited twice while walking predecessors",
            loaded.manifest.name
        );
    }

    let signature_verified = match (
        loaded.receipt_envelope_path(),
        loaded.receipt_verify_key_path(),
    ) {
        (Some(envelope), Some(key)) => {
            verify_dsse_envelope(&envelope, &key)?;
            true
        }
        (None, None) => false,
        _ => bail!(
            "BundleLineageSignatureMissing: bundle `{}` is missing either its receipt envelope or verify key",
            loaded.manifest.name
        ),
    };

    nodes.push(LineageNode {
        name: loaded.manifest.name.clone(),
        path: loaded.bundle_dir.display().to_string(),
        depth,
        signature_verified,
        predecessor_count: loaded.manifest.lineage.predecessors.len(),
    });

    for predecessor in &loaded.manifest.lineage.predecessors {
        let pred_manifest = LoadedManifest::load(&loaded.resolve(&predecessor.path))
            .with_context(|| {
                format!(
                    "load predecessor `{}` for bundle `{}`",
                    predecessor.name, loaded.manifest.name
                )
            })?;
        walk_bundle_inner(&pred_manifest, visited, nodes, depth + 1)?;
    }

    Ok(())
}

#[derive(Debug, Serialize)]
struct LineageReport {
    root_bundle: String,
    nodes: Vec<LineageNode>,
}

#[derive(Debug, Serialize)]
struct LineageNode {
    name: String,
    path: String,
    depth: usize,
    signature_verified: bool,
    predecessor_count: usize,
}
