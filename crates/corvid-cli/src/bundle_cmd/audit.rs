use anyhow::Result;
use corvid_abi::{read_descriptor_from_path, AbiOwnershipMode};
use std::path::Path;

use super::manifest::LoadedManifest;

pub fn run_audit(bundle: &Path, question: Option<&str>, json: bool) -> Result<u8> {
    let loaded = LoadedManifest::load(bundle)?;
    let abi = read_descriptor_from_path(&loaded.descriptor_path())?;
    let question = question.unwrap_or("summarize the approval, ownership, and signature status");
    let answer = answer_question(&loaded, &abi, question);
    if json {
        println!("{}", serde_json::to_string_pretty(&answer)?);
    } else {
        println!("question: {}", answer.question);
        println!("answer: {}", answer.answer);
        println!("citations:");
        for citation in answer.citations {
            println!("  - {citation}");
        }
    }
    Ok(0)
}

#[derive(serde::Serialize)]
struct AuditAnswer {
    question: String,
    answer: String,
    citations: Vec<String>,
}

fn answer_question(
    loaded: &LoadedManifest,
    abi: &corvid_abi::CorvidAbi,
    question: &str,
) -> AuditAnswer {
    let lower = question.to_ascii_lowercase();
    if lower.contains("dangerous") || lower.contains("approval") {
        let gated = abi
            .agents
            .iter()
            .filter(|agent| agent.attributes.dangerous || agent.approval_contract.required)
            .map(|agent| agent.name.clone())
            .collect::<Vec<_>>();
        let answer = if gated.is_empty() {
            "no exported agents require approval or expose dangerous behavior".to_string()
        } else {
            format!("approval-gated agents: {}", gated.join(", "))
        };
        return AuditAnswer {
            question: question.to_string(),
            answer,
            citations: vec![
                loaded.descriptor_path().display().to_string(),
                loaded.manifest_path.display().to_string(),
            ],
        };
    }
    if lower.contains("signature") {
        let answer = if loaded.receipt_envelope_path().is_some() && loaded.receipt_verify_key_path().is_some() {
            "bundle includes a signed receipt envelope and a verifying key for offline signature validation".to_string()
        } else {
            "bundle has no signed receipt configured in its manifest".to_string()
        };
        return AuditAnswer {
            question: question.to_string(),
            answer,
            citations: vec![loaded.manifest_path.display().to_string()],
        };
    }
    if lower.contains("ownership") {
        let mut findings = Vec::new();
        for agent in &abi.agents {
            if let Some(ownership) = &agent.return_ownership {
                let rendered = match ownership.mode {
                    AbiOwnershipMode::Owned => "@owned".to_string(),
                    AbiOwnershipMode::Borrowed => "@borrowed".to_string(),
                    AbiOwnershipMode::Shared => "@shared".to_string(),
                    AbiOwnershipMode::Static => "@static".to_string(),
                };
                findings.push(format!("{} return {}", agent.name, rendered));
            }
        }
        return AuditAnswer {
            question: question.to_string(),
            answer: if findings.is_empty() {
                "no explicit ownership annotations found in exported returns".to_string()
            } else {
                findings.join("; ")
            },
            citations: vec![loaded.descriptor_path().display().to_string()],
        };
    }

    AuditAnswer {
        question: question.to_string(),
        answer: format!(
            "bundle `{}` targets {} and ships {} traces, {} exported agents, and {} signed receipt(s)",
            loaded.manifest.name,
            loaded.manifest.target_triple,
            loaded.manifest.traces.len(),
            abi.agents.len(),
            if loaded.receipt_envelope_path().is_some() { 1 } else { 0 }
        ),
        citations: vec![
            loaded.manifest_path.display().to_string(),
            loaded.descriptor_path().display().to_string(),
        ],
    }
}
