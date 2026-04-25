//! Machine-checkable proof replay for proof-carrying dimensions.
//!
//! Corvid's own dimension law checker is the mandatory baseline. This
//! module owns the optional external proof-assistant bridge: if a
//! dimension declares a `.lean` or `.v` proof, replay it through Lean or
//! Coq and fail closed when the proof cannot be checked.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const PROOF_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProofReplayStatus {
    Passed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProofReplayResult {
    pub dimension: String,
    pub proof_path: PathBuf,
    pub assistant: String,
    pub status: ProofReplayStatus,
    pub message: String,
}

impl ProofReplayResult {
    pub fn failed(&self) -> bool {
        self.status == ProofReplayStatus::Failed
    }
}

pub fn replay_dimension_proof(dimension: &str, proof_path: &Path) -> ProofReplayResult {
    let Some((assistant, command)) = assistant_for_path(proof_path) else {
        return ProofReplayResult {
            dimension: dimension.to_string(),
            proof_path: proof_path.to_path_buf(),
            assistant: "unsupported".into(),
            status: ProofReplayStatus::Failed,
            message: "proof path must end in `.lean` or `.v`".into(),
        };
    };

    if !proof_path.exists() {
        return ProofReplayResult {
            dimension: dimension.to_string(),
            proof_path: proof_path.to_path_buf(),
            assistant: assistant.into(),
            status: ProofReplayStatus::Failed,
            message: format!("proof file `{}` does not exist", proof_path.display()),
        };
    }

    run_proof_assistant(dimension, proof_path, assistant, &command)
}

fn assistant_for_path(proof_path: &Path) -> Option<(&'static str, String)> {
    match proof_path.extension().and_then(|ext| ext.to_str()) {
        Some("lean") => Some((
            "lean",
            std::env::var("CORVID_LEAN").unwrap_or_else(|_| "lean".into()),
        )),
        Some("v") => Some((
            "coq",
            std::env::var("CORVID_COQC").unwrap_or_else(|_| "coqc".into()),
        )),
        _ => None,
    }
}

fn run_proof_assistant(
    dimension: &str,
    proof_path: &Path,
    assistant: &str,
    command: &str,
) -> ProofReplayResult {
    let mut child = match Command::new(command)
        .arg(proof_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(err) => {
            return ProofReplayResult {
                dimension: dimension.to_string(),
                proof_path: proof_path.to_path_buf(),
                assistant: assistant.into(),
                status: ProofReplayStatus::Failed,
                message: format!(
                    "could not start `{command}` for {assistant} proof replay: {err}; \
                     install {assistant} or set {}",
                    if assistant == "lean" {
                        "CORVID_LEAN"
                    } else {
                        "CORVID_COQC"
                    }
                ),
            }
        }
    };

    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() >= PROOF_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return ProofReplayResult {
                    dimension: dimension.to_string(),
                    proof_path: proof_path.to_path_buf(),
                    assistant: assistant.into(),
                    status: ProofReplayStatus::Failed,
                    message: format!("proof replay timed out after {}s", PROOF_TIMEOUT.as_secs()),
                };
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                return ProofReplayResult {
                    dimension: dimension.to_string(),
                    proof_path: proof_path.to_path_buf(),
                    assistant: assistant.into(),
                    status: ProofReplayStatus::Failed,
                    message: format!("proof replay failed while waiting for `{command}`: {err}"),
                };
            }
        }
    }

    let output = match child.wait_with_output() {
        Ok(output) => output,
        Err(err) => {
            return ProofReplayResult {
                dimension: dimension.to_string(),
                proof_path: proof_path.to_path_buf(),
                assistant: assistant.into(),
                status: ProofReplayStatus::Failed,
                message: format!("could not collect `{command}` output: {err}"),
            }
        }
    };

    if output.status.success() {
        ProofReplayResult {
            dimension: dimension.to_string(),
            proof_path: proof_path.to_path_buf(),
            assistant: assistant.into(),
            status: ProofReplayStatus::Passed,
            message: "proof replay passed".into(),
        }
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = if !stderr.trim().is_empty() {
            stderr.trim()
        } else {
            stdout.trim()
        };
        ProofReplayResult {
            dimension: dimension.to_string(),
            proof_path: proof_path.to_path_buf(),
            assistant: assistant.into(),
            status: ProofReplayStatus::Failed,
            message: format!(
                "{assistant} proof replay exited with {}; {}",
                output.status,
                detail
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn unsupported_proof_extension_fails_closed() {
        let tmp = TempDir::new().unwrap();
        let proof = tmp.path().join("freshness.txt");
        std::fs::write(&proof, "not a proof").unwrap();
        let result = replay_dimension_proof("freshness", &proof);
        assert!(result.failed(), "{result:?}");
        assert!(result.message.contains(".lean") && result.message.contains(".v"));
    }

    #[test]
    fn missing_lean_proof_fails_before_invoking_tool() {
        let tmp = TempDir::new().unwrap();
        let proof = tmp.path().join("freshness.lean");
        let result = replay_dimension_proof("freshness", &proof);
        assert!(result.failed(), "{result:?}");
        assert!(result.message.contains("does not exist"));
    }
}
