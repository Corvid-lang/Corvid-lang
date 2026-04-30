use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const CONNECTOR_MANIFEST_SCHEMA: &str = "corvid.connector.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorManifest {
    pub schema: String,
    pub name: String,
    pub provider: String,
    #[serde(default)]
    pub mode: Vec<ConnectorMode>,
    #[serde(default)]
    pub scope: Vec<ConnectorScope>,
    #[serde(default)]
    pub rate_limit: Vec<RateLimitDeclaration>,
    #[serde(default)]
    pub redaction: Vec<RedactionRule>,
    #[serde(default)]
    pub replay: Vec<ReplayDeclaration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorMode {
    Mock,
    Replay,
    Real,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorScope {
    pub id: String,
    pub provider_scope: String,
    #[serde(default)]
    pub data_classes: Vec<String>,
    #[serde(default)]
    pub effects: Vec<String>,
    pub approval: ConnectorScopeApproval,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorScopeApproval {
    None,
    Required,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RateLimitDeclaration {
    pub key: String,
    pub limit: u64,
    pub window_ms: u64,
    pub retry_after: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedactionRule {
    pub field: String,
    pub strategy: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayDeclaration {
    pub operation: String,
    pub policy: ConnectorReplayPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorReplayPolicy {
    RecordRead,
    QuarantineWrite,
    DeterministicMock,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorValidationReport {
    pub valid: bool,
    pub diagnostics: Vec<ConnectorManifestError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectorManifestError {
    UnsupportedSchema(String),
    MissingName,
    MissingProvider,
    MissingMode(ConnectorMode),
    DuplicateScope(String),
    MissingProviderScope(String),
    MissingDataClasses(String),
    MissingEffects(String),
    WriteScopeWithoutApproval(String),
    InvalidRateLimit(String),
    MissingSensitiveRedaction(String),
    MissingReplayPolicy(String),
}

impl std::fmt::Display for ConnectorManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedSchema(schema) => write!(f, "unsupported connector schema `{schema}`"),
            Self::MissingName => write!(f, "connector manifest requires name"),
            Self::MissingProvider => write!(f, "connector manifest requires provider"),
            Self::MissingMode(mode) => write!(f, "connector manifest missing mode `{mode:?}`"),
            Self::DuplicateScope(scope) => write!(f, "duplicate connector scope `{scope}`"),
            Self::MissingProviderScope(scope) => {
                write!(f, "scope `{scope}` requires provider_scope")
            }
            Self::MissingDataClasses(scope) => write!(f, "scope `{scope}` requires data_classes"),
            Self::MissingEffects(scope) => write!(f, "scope `{scope}` requires effects"),
            Self::WriteScopeWithoutApproval(scope) => {
                write!(f, "write scope `{scope}` requires approval")
            }
            Self::InvalidRateLimit(key) => {
                write!(f, "rate limit `{key}` must have positive limit/window")
            }
            Self::MissingSensitiveRedaction(class) => {
                write!(
                    f,
                    "sensitive data class `{class}` requires a redaction rule"
                )
            }
            Self::MissingReplayPolicy(scope) => {
                write!(f, "scope `{scope}` requires replay policy")
            }
        }
    }
}

impl std::error::Error for ConnectorManifestError {}

pub fn parse_connector_manifest(source: &str) -> Result<ConnectorManifest, toml::de::Error> {
    toml::from_str(source)
}

pub fn validate_connector_manifest(manifest: &ConnectorManifest) -> ConnectorValidationReport {
    let mut diagnostics = Vec::new();
    if manifest.schema != CONNECTOR_MANIFEST_SCHEMA {
        diagnostics.push(ConnectorManifestError::UnsupportedSchema(
            manifest.schema.clone(),
        ));
    }
    if manifest.name.trim().is_empty() {
        diagnostics.push(ConnectorManifestError::MissingName);
    }
    if manifest.provider.trim().is_empty() {
        diagnostics.push(ConnectorManifestError::MissingProvider);
    }
    for required in [
        ConnectorMode::Mock,
        ConnectorMode::Replay,
        ConnectorMode::Real,
    ] {
        if !manifest.mode.contains(&required) {
            diagnostics.push(ConnectorManifestError::MissingMode(required));
        }
    }

    let mut scope_ids = BTreeSet::new();
    for scope in &manifest.scope {
        if !scope_ids.insert(scope.id.clone()) {
            diagnostics.push(ConnectorManifestError::DuplicateScope(scope.id.clone()));
        }
        if scope.provider_scope.trim().is_empty() {
            diagnostics.push(ConnectorManifestError::MissingProviderScope(
                scope.id.clone(),
            ));
        }
        if scope.data_classes.is_empty() {
            diagnostics.push(ConnectorManifestError::MissingDataClasses(scope.id.clone()));
        }
        if scope.effects.is_empty() {
            diagnostics.push(ConnectorManifestError::MissingEffects(scope.id.clone()));
        }
        if scope_is_write(scope) && scope.approval != ConnectorScopeApproval::Required {
            diagnostics.push(ConnectorManifestError::WriteScopeWithoutApproval(
                scope.id.clone(),
            ));
        }
        if !scope_has_replay_policy(scope, &manifest.replay) {
            diagnostics.push(ConnectorManifestError::MissingReplayPolicy(
                scope.id.clone(),
            ));
        }
    }

    for rate_limit in &manifest.rate_limit {
        if rate_limit.key.trim().is_empty() || rate_limit.limit == 0 || rate_limit.window_ms == 0 {
            diagnostics.push(ConnectorManifestError::InvalidRateLimit(
                rate_limit.key.clone(),
            ));
        }
    }

    let has_redaction = !manifest.redaction.is_empty();
    for data_class in manifest
        .scope
        .iter()
        .flat_map(|scope| scope.data_classes.iter())
        .filter(|class| data_class_is_sensitive(class))
    {
        if !has_redaction {
            diagnostics.push(ConnectorManifestError::MissingSensitiveRedaction(
                data_class.clone(),
            ));
        }
    }

    ConnectorValidationReport {
        valid: diagnostics.is_empty(),
        diagnostics,
    }
}

fn scope_is_write(scope: &ConnectorScope) -> bool {
    scope
        .effects
        .iter()
        .any(|effect| effect.contains(".write") || effect.starts_with("send_"))
}

fn scope_has_replay_policy(scope: &ConnectorScope, replay: &[ReplayDeclaration]) -> bool {
    let operation = scope_operation(&scope.id);
    replay
        .iter()
        .any(|declaration| declaration.operation == operation)
}

fn scope_operation(scope_id: &str) -> &str {
    scope_id.rsplit('.').next().unwrap_or(scope_id)
}

fn data_class_is_sensitive(data_class: &str) -> bool {
    let lower = data_class.to_ascii_lowercase();
    lower.contains("body")
        || lower.contains("secret")
        || lower.contains("token")
        || lower.contains("private")
        || lower.contains("external_recipient")
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"
schema = "corvid.connector.v1"
name = "gmail"
provider = "google"
mode = ["mock", "replay", "real"]

[[scope]]
id = "gmail.read_metadata"
provider_scope = "https://www.googleapis.com/auth/gmail.metadata"
data_classes = ["email_metadata"]
effects = ["network.read"]
approval = "none"

[[scope]]
id = "gmail.send"
provider_scope = "https://www.googleapis.com/auth/gmail.send"
data_classes = ["email_metadata", "email_body", "external_recipient"]
effects = ["network.write", "send_email"]
approval = "required"

[[rate_limit]]
key = "user_id"
limit = 250
window_ms = 1000
retry_after = "provider_header"

[[redaction]]
field = "message.body"
strategy = "hash_and_drop"

[[replay]]
operation = "read_metadata"
policy = "record_read"

[[replay]]
operation = "send"
policy = "quarantine_write"
"#;

    #[test]
    fn parses_and_accepts_valid_connector_manifest() {
        let manifest = parse_connector_manifest(VALID).unwrap();
        let report = validate_connector_manifest(&manifest);
        assert!(report.valid, "{report:?}");
    }

    #[test]
    fn rejects_unsafe_write_without_approval_replay_or_redaction() {
        let mut manifest = parse_connector_manifest(VALID).unwrap();
        manifest.scope[1].approval = ConnectorScopeApproval::None;
        manifest.redaction.clear();
        manifest.replay.retain(|entry| entry.operation != "send");

        let report = validate_connector_manifest(&manifest);
        assert!(!report.valid);
        assert!(report.diagnostics.iter().any(|diagnostic| matches!(
            diagnostic,
            ConnectorManifestError::WriteScopeWithoutApproval(scope) if scope == "gmail.send"
        )));
        assert!(report.diagnostics.iter().any(|diagnostic| matches!(
            diagnostic,
            ConnectorManifestError::MissingReplayPolicy(scope) if scope == "gmail.send"
        )));
        assert!(report.diagnostics.iter().any(|diagnostic| matches!(
            diagnostic,
            ConnectorManifestError::MissingSensitiveRedaction(class) if class == "email_body"
        )));
    }

    #[test]
    fn rejects_missing_modes_duplicate_scopes_and_invalid_rate_limits() {
        let mut manifest = parse_connector_manifest(VALID).unwrap();
        manifest.mode = vec![ConnectorMode::Mock];
        manifest.scope.push(manifest.scope[0].clone());
        manifest.rate_limit[0].limit = 0;

        let report = validate_connector_manifest(&manifest);
        assert!(!report.valid);
        assert!(report
            .diagnostics
            .contains(&ConnectorManifestError::MissingMode(ConnectorMode::Replay)));
        assert!(report
            .diagnostics
            .contains(&ConnectorManifestError::MissingMode(ConnectorMode::Real)));
        assert!(report.diagnostics.iter().any(|diagnostic| matches!(
            diagnostic,
            ConnectorManifestError::DuplicateScope(scope) if scope == "gmail.read_metadata"
        )));
        assert!(report.diagnostics.iter().any(|diagnostic| matches!(
            diagnostic,
            ConnectorManifestError::InvalidRateLimit(key) if key == "user_id"
        )));
    }
}
