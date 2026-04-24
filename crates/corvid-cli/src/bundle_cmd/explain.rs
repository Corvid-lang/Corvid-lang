use anyhow::Result;
use corvid_abi::read_descriptor_from_path;
use serde::Serialize;
use std::path::Path;

use super::manifest::LoadedManifest;

pub fn run_explain(bundle: &Path, json: bool) -> Result<u8> {
    let loaded = LoadedManifest::load(bundle)?;
    let abi = read_descriptor_from_path(&loaded.descriptor_path())?;
    let explanation = BundleExplanation {
        name: loaded.manifest.name.clone(),
        target_triple: loaded.manifest.target_triple.clone(),
        trace_count: loaded.manifest.traces.len(),
        exported_agents: abi.agents.iter().map(|agent| agent.name.clone()).collect(),
        signed_receipt: loaded.receipt_envelope_path().is_some() && loaded.receipt_verify_key_path().is_some(),
        citations: vec![
            loaded.manifest_path.display().to_string(),
            loaded.descriptor_path().display().to_string(),
        ],
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&explanation)?);
    } else {
        println!("bundle: {}", explanation.name);
        println!("target: {}", explanation.target_triple);
        println!("traces: {}", explanation.trace_count);
        println!("exported agents: {}", explanation.exported_agents.join(", "));
        println!("signed receipt: {}", explanation.signed_receipt);
        println!("citations:");
        for citation in explanation.citations {
            println!("  - {citation}");
        }
    }
    Ok(0)
}

#[derive(Debug, Serialize)]
struct BundleExplanation {
    name: String,
    target_triple: String,
    trace_count: usize,
    exported_agents: Vec<String>,
    signed_receipt: bool,
    citations: Vec<String>,
}
