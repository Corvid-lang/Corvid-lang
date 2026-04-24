use anyhow::Result;
use corvid_abi::{read_descriptor_from_path, AbiOwnershipMode};
use serde::Serialize;
use std::path::Path;

use super::manifest::{sha256_file, LoadedManifest};

pub fn run_diff(old_bundle: &Path, new_bundle: &Path, json: bool) -> Result<u8> {
    let old_loaded = LoadedManifest::load(old_bundle)?;
    let new_loaded = LoadedManifest::load(new_bundle)?;
    let old_abi = read_descriptor_from_path(&old_loaded.descriptor_path())?;
    let new_abi = read_descriptor_from_path(&new_loaded.descriptor_path())?;

    let mut ownership_changes = Vec::new();
    for old_agent in &old_abi.agents {
        let Some(new_agent) = new_abi.agents.iter().find(|candidate| candidate.name == old_agent.name) else {
            continue;
        };
        let old_return = render_ownership(old_agent.return_ownership.as_ref());
        let new_return = render_ownership(new_agent.return_ownership.as_ref());
        if old_return != new_return {
            ownership_changes.push(OwnershipChange {
                agent: old_agent.name.clone(),
                position: "return".to_string(),
                from: old_return,
                to: new_return,
            });
        }
        for (index, old_param) in old_agent.params.iter().enumerate() {
            let Some(new_param) = new_agent.params.get(index) else {
                continue;
            };
            let old_mode = render_ownership(old_param.ownership.as_ref());
            let new_mode = render_ownership(new_param.ownership.as_ref());
            if old_mode != new_mode {
                ownership_changes.push(OwnershipChange {
                    agent: old_agent.name.clone(),
                    position: format!("arg[{index}]"),
                    from: old_mode,
                    to: new_mode,
                });
            }
        }
    }

    let diff = BundleDiff {
        old_name: old_loaded.manifest.name.clone(),
        new_name: new_loaded.manifest.name.clone(),
        target_changed: old_loaded.manifest.target_triple != new_loaded.manifest.target_triple,
        descriptor_hash_changed: sha256_file(&old_loaded.descriptor_path())?
            != sha256_file(&new_loaded.descriptor_path())?,
        library_hash_changed: sha256_file(&old_loaded.library_path())?
            != sha256_file(&new_loaded.library_path())?,
        rust_bindings_hash_changed: super::manifest::sha256_dir(&old_loaded.bindings_rust_dir())?
            != super::manifest::sha256_dir(&new_loaded.bindings_rust_dir())?,
        python_bindings_hash_changed: super::manifest::sha256_dir(&old_loaded.bindings_python_dir())?
            != super::manifest::sha256_dir(&new_loaded.bindings_python_dir())?,
        trace_count_delta: new_loaded.manifest.traces.len() as isize - old_loaded.manifest.traces.len() as isize,
        ownership_changes,
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&diff)?);
    } else {
        println!("bundle diff: {} -> {}", diff.old_name, diff.new_name);
        println!("target_changed={}", diff.target_changed);
        println!("descriptor_hash_changed={}", diff.descriptor_hash_changed);
        println!("library_hash_changed={}", diff.library_hash_changed);
        println!("rust_bindings_hash_changed={}", diff.rust_bindings_hash_changed);
        println!(
            "python_bindings_hash_changed={}",
            diff.python_bindings_hash_changed
        );
        println!("trace_count_delta={}", diff.trace_count_delta);
        if diff.ownership_changes.is_empty() {
            println!("ownership_changes=none");
        } else {
            println!("ownership_changes:");
            for change in &diff.ownership_changes {
                println!(
                    "  - {} {}: {} -> {}",
                    change.agent, change.position, change.from, change.to
                );
            }
        }
    }

    Ok(0)
}

#[derive(Debug, Serialize)]
struct BundleDiff {
    old_name: String,
    new_name: String,
    target_changed: bool,
    descriptor_hash_changed: bool,
    library_hash_changed: bool,
    rust_bindings_hash_changed: bool,
    python_bindings_hash_changed: bool,
    trace_count_delta: isize,
    ownership_changes: Vec<OwnershipChange>,
}

#[derive(Debug, Serialize)]
struct OwnershipChange {
    agent: String,
    position: String,
    from: String,
    to: String,
}

fn render_ownership(ownership: Option<&corvid_abi::AbiOwnership>) -> String {
    match ownership {
        Some(ownership) => match ownership.mode {
            AbiOwnershipMode::Owned => "@owned".to_string(),
            AbiOwnershipMode::Borrowed => {
                if let Some(lifetime) = ownership.lifetime.as_deref() {
                    if lifetime == "call" {
                        "@borrowed".to_string()
                    } else {
                        format!("@borrowed<'{lifetime}>")
                    }
                } else {
                    "@borrowed".to_string()
                }
            }
            AbiOwnershipMode::Shared => "@shared".to_string(),
            AbiOwnershipMode::Static => "@static".to_string(),
        },
        None => "@owned".to_string(),
    }
}
