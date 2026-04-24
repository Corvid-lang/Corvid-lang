use anyhow::{bail, Result};
use corvid_abi::read_descriptor_from_path;
use serde::Serialize;
use std::path::Path;

use super::manifest::LoadedManifest;

pub fn run_report(bundle: &Path, format: &str, json: bool) -> Result<u8> {
    if format != "soc2" {
        bail!("unsupported bundle report format `{format}`; valid: soc2");
    }
    let loaded = LoadedManifest::load(bundle)?;
    let abi = read_descriptor_from_path(&loaded.descriptor_path())?;
    let approval_gated = abi
        .agents
        .iter()
        .filter(|agent| agent.attributes.dangerous || agent.approval_contract.required)
        .map(|agent| agent.name.clone())
        .collect::<Vec<_>>();
    let report = Soc2BundleReport {
        bundle: loaded.manifest.name.clone(),
        controls: vec![
            Soc2Control {
                control_id: "CC7.2".to_string(),
                title: "Change management and signed evidence".to_string(),
                evidence: vec![
                    loaded.descriptor_path().display().to_string(),
                    loaded
                        .receipt_envelope_path()
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| "manifest has no signed receipt".to_string()),
                ],
            },
            Soc2Control {
                control_id: "CC6.1".to_string(),
                title: "Approval-gated dangerous behavior".to_string(),
                evidence: if approval_gated.is_empty() {
                    vec!["no exported dangerous agents".to_string()]
                } else {
                    approval_gated
                },
            },
            Soc2Control {
                control_id: "A1.2".to_string(),
                title: "Deterministic rebuild and binding projection".to_string(),
                evidence: vec![
                    "corvid bundle verify --rebuild".to_string(),
                    loaded.library_path().display().to_string(),
                    loaded.bindings_rust_dir().display().to_string(),
                    loaded.bindings_python_dir().display().to_string(),
                ],
            },
        ],
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("# SOC2-style control mapping");
        println!();
        println!("bundle: {}", report.bundle);
        println!();
        for control in report.controls {
            println!("## {} {}", control.control_id, control.title);
            for evidence in control.evidence {
                println!("- {}", evidence);
            }
            println!();
        }
    }
    Ok(0)
}

#[derive(Debug, Serialize)]
struct Soc2BundleReport {
    bundle: String,
    controls: Vec<Soc2Control>,
}

#[derive(Debug, Serialize)]
struct Soc2Control {
    control_id: String,
    title: String,
    evidence: Vec<String>,
}
