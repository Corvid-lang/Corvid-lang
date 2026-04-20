use crate::alerts::{Alert, AlertKind};
use crate::config::EnrollmentConfig;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AckMetadata {
    pub reason: String,
    pub observed_commit_sha: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollmentAction {
    pub trace_path: PathBuf,
    pub enrolled_path: PathBuf,
    pub ack_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct EnrollmentManager {
    config: EnrollmentConfig,
}

impl EnrollmentManager {
    pub fn new(config: EnrollmentConfig) -> Self {
        Self { config }
    }

    pub fn maybe_auto_enroll(&self, alert: &Alert) -> Result<Option<EnrollmentAction>> {
        if self.config.auto_enroll {
            return self.enroll(&alert.trace_path, "auto_enroll=true", None).map(Some);
        }
        match alert.kind {
            AlertKind::Dimension => {
                let dimension = alert
                    .payload
                    .get("dimension")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                if dimension == "trust" && self.config.auto_enroll_on_trust_drop {
                    return self
                        .enroll(&alert.trace_path, "auto-enroll trust drop", None)
                        .map(Some);
                }
                if dimension == "budget" && self.config.auto_enroll_on_budget_overrun {
                    return self
                        .enroll(&alert.trace_path, "auto-enroll budget overrun", None)
                        .map(Some);
                }
            }
            _ => {}
        }
        Ok(None)
    }

    pub fn enroll(
        &self,
        trace_path: &Path,
        reason: &str,
        observed_commit_sha: Option<String>,
    ) -> Result<EnrollmentAction> {
        std::fs::create_dir_all(&self.config.target_corpus_dir).with_context(|| {
            format!(
                "failed to create enrollment corpus dir `{}`",
                self.config.target_corpus_dir.display()
            )
        })?;
        let file_name = trace_path
            .file_name()
            .context("trace path has no file name")?;
        let enrolled_path = self.config.target_corpus_dir.join(file_name);
        std::fs::copy(trace_path, &enrolled_path).with_context(|| {
            format!(
                "failed to copy `{}` into `{}`",
                trace_path.display(),
                enrolled_path.display()
            )
        })?;

        let ack_path = enrolled_path.with_extension("ack.toml");
        let ack = AckMetadata {
            reason: reason.into(),
            observed_commit_sha,
        };
        std::fs::write(&ack_path, toml::to_string_pretty(&ack)?).with_context(|| {
            format!("failed to write sidecar ack `{}`", ack_path.display())
        })?;

        Ok(EnrollmentAction {
            trace_path: trace_path.to_path_buf(),
            enrolled_path,
            ack_path,
        })
    }
}
