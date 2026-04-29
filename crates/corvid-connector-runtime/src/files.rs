use crate::{
    parse_connector_manifest, ConnectorAuthState, ConnectorManifest, ConnectorRequest,
    ConnectorRuntime, ConnectorRuntimeError, ConnectorRuntimeMode,
};
use serde::{Deserialize, Serialize};

pub const FILE_CONNECTOR_MANIFEST: &str = r#"
schema = "corvid.connector.v1"
name = "files"
provider = "local_files"
mode = ["mock", "replay", "real"]

[[scope]]
id = "files.index"
provider_scope = "files.read"
data_classes = ["file_metadata"]
effects = ["filesystem.read"]
approval = "none"

[[scope]]
id = "files.read"
provider_scope = "files.read"
data_classes = ["file_metadata", "file_snippet"]
effects = ["filesystem.read"]
approval = "none"

[[scope]]
id = "files.write"
provider_scope = "files.write"
data_classes = ["file_metadata", "file_snippet"]
effects = ["filesystem.write", "file.write"]
approval = "required"

[[rate_limit]]
key = "tenant_user"
limit = 1000
window_ms = 1000
retry_after = "local_window"

[[redaction]]
field = "snippet.text"
strategy = "hash_and_drop"

[[replay]]
operation = "index"
policy = "record_read"

[[replay]]
operation = "read"
policy = "record_read"

[[replay]]
operation = "write"
policy = "quarantine_write"

[[replay]]
operation = "delete"
policy = "quarantine_write"
"#;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileIndexRequest {
    pub root_id: String,
    pub glob: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileReadRequest {
    pub root_id: String,
    pub path: String,
    pub max_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileMetadata {
    pub root_id: String,
    pub path: String,
    pub size_bytes: u64,
    pub modified_ms: u64,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSnippet {
    pub root_id: String,
    pub path: String,
    pub content_hash: String,
    pub byte_start: u64,
    pub byte_end: u64,
    pub provenance_id: String,
    pub text_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileWriteKind {
    Create,
    Update,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileWriteRequest {
    pub root_id: String,
    pub path: String,
    pub content_hash: String,
    pub kind: FileWriteKind,
    pub approval_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileWriteReceipt {
    pub root_id: String,
    pub path: String,
    pub content_hash: String,
    pub approval_id: String,
    pub replay_key: String,
    pub provenance_id: String,
}

#[derive(Debug, Clone)]
pub struct FileConnector {
    runtime: ConnectorRuntime,
}

impl FileConnector {
    pub fn new(
        auth: ConnectorAuthState,
        mode: ConnectorRuntimeMode,
    ) -> Result<Self, toml::de::Error> {
        Ok(Self {
            runtime: ConnectorRuntime::new(file_manifest()?, auth, mode),
        })
    }

    pub fn insert_mock(&mut self, operation: impl Into<String>, payload: serde_json::Value) {
        self.runtime.insert_mock(operation, payload);
    }

    pub fn index(
        &mut self,
        request: FileIndexRequest,
        now_ms: u64,
    ) -> Result<Vec<FileMetadata>, ConnectorRuntimeError> {
        let replay_key = format!("files:index:{}:{}", request.root_id, stable(&request.glob));
        let response = self.runtime.execute(ConnectorRequest {
            scope_id: "files.index".to_string(),
            operation: "index".to_string(),
            payload: serde_json::to_value(&request).unwrap_or_default(),
            approval_id: String::new(),
            replay_key,
            now_ms,
        })?;
        Ok(serde_json::from_value(response.payload).unwrap_or_default())
    }

    pub fn read(
        &mut self,
        request: FileReadRequest,
        now_ms: u64,
    ) -> Result<FileSnippet, ConnectorRuntimeError> {
        let replay_key = format!("files:read:{}:{}", request.root_id, stable(&request.path));
        let response = self.runtime.execute(ConnectorRequest {
            scope_id: "files.read".to_string(),
            operation: "read".to_string(),
            payload: serde_json::to_value(&request).unwrap_or_default(),
            approval_id: String::new(),
            replay_key,
            now_ms,
        })?;
        serde_json::from_value(response.payload)
            .map_err(|err| ConnectorRuntimeError::MissingMock(err.to_string()))
    }

    pub fn write(
        &mut self,
        request: FileWriteRequest,
        now_ms: u64,
    ) -> Result<FileWriteReceipt, ConnectorRuntimeError> {
        let operation = match request.kind {
            FileWriteKind::Create | FileWriteKind::Update => "write",
            FileWriteKind::Delete => "delete",
        };
        let replay_key = format!(
            "files:{}:{}:{}:{}",
            operation,
            request.root_id,
            stable(&request.path),
            request.content_hash
        );
        let approval_id = request.approval_id.clone();
        let response = self.runtime.execute(ConnectorRequest {
            scope_id: "files.write".to_string(),
            operation: operation.to_string(),
            payload: serde_json::to_value(&request).unwrap_or_default(),
            approval_id,
            replay_key,
            now_ms,
        })?;
        serde_json::from_value(response.payload)
            .map_err(|err| ConnectorRuntimeError::MissingMock(err.to_string()))
    }
}

pub fn file_manifest() -> Result<ConnectorManifest, toml::de::Error> {
    parse_connector_manifest(FILE_CONNECTOR_MANIFEST)
}

fn stable(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validate_connector_manifest;

    fn auth() -> ConnectorAuthState {
        ConnectorAuthState::new(
            "tenant-1",
            "actor-1",
            "token-1",
            ["files.index", "files.read", "files.write"],
            10_000,
        )
    }

    #[test]
    fn file_manifest_validates_read_contract() {
        let manifest = file_manifest().unwrap();
        let report = validate_connector_manifest(&manifest);
        assert!(report.valid, "{report:?}");
    }

    #[test]
    fn file_index_and_read_work_in_mock_mode_with_provenance() {
        let mut connector = FileConnector::new(auth(), ConnectorRuntimeMode::Mock).unwrap();
        let metadata = FileMetadata {
            root_id: "docs".to_string(),
            path: "notes/today.md".to_string(),
            size_bytes: 42,
            modified_ms: 100,
            content_hash: "sha256:file".to_string(),
        };
        let snippet = FileSnippet {
            root_id: "docs".to_string(),
            path: "notes/today.md".to_string(),
            content_hash: "sha256:file".to_string(),
            byte_start: 0,
            byte_end: 20,
            provenance_id: "file://docs/notes/today.md#sha256:file:0-20".to_string(),
            text_fingerprint: "sha256:snippet".to_string(),
        };
        connector.insert_mock("index", serde_json::json!([metadata.clone()]));
        connector.insert_mock("read", serde_json::json!(snippet.clone()));

        assert_eq!(
            connector
                .index(
                    FileIndexRequest {
                        root_id: "docs".to_string(),
                        glob: "**/*.md".to_string(),
                    },
                    1,
                )
                .unwrap(),
            vec![metadata]
        );
        let read = connector
            .read(
                FileReadRequest {
                    root_id: "docs".to_string(),
                    path: "notes/today.md".to_string(),
                    max_bytes: 1024,
                },
                2,
            )
            .unwrap();
        assert_eq!(read, snippet);
        assert!(read.provenance_id.contains("sha256:file"));
    }

    #[test]
    fn file_writes_require_approval_and_record_provenance() {
        let mut connector = FileConnector::new(auth(), ConnectorRuntimeMode::Mock).unwrap();
        connector.insert_mock(
            "write",
            serde_json::json!({
                "root_id": "docs",
                "path": "notes/today.md",
                "content_hash": "sha256:new",
                "approval_id": "approval-1",
                "replay_key": "files:write:docs:notes-today-md:sha256:new",
                "provenance_id": "file://docs/notes/today.md#sha256:new"
            }),
        );

        let missing = connector
            .write(
                FileWriteRequest {
                    root_id: "docs".to_string(),
                    path: "notes/today.md".to_string(),
                    content_hash: "sha256:new".to_string(),
                    kind: FileWriteKind::Update,
                    approval_id: String::new(),
                },
                1,
            )
            .unwrap_err();
        assert!(
            matches!(missing, ConnectorRuntimeError::ApprovalRequired(scope) if scope == "files.write")
        );

        let receipt = connector
            .write(
                FileWriteRequest {
                    root_id: "docs".to_string(),
                    path: "notes/today.md".to_string(),
                    content_hash: "sha256:new".to_string(),
                    kind: FileWriteKind::Update,
                    approval_id: "approval-1".to_string(),
                },
                2,
            )
            .unwrap();
        assert_eq!(receipt.approval_id, "approval-1");
        assert!(receipt.provenance_id.contains("sha256:new"));
    }

    #[test]
    fn file_replay_quarantines_writes() {
        let mut connector = FileConnector::new(auth(), ConnectorRuntimeMode::Replay).unwrap();
        let err = connector
            .write(
                FileWriteRequest {
                    root_id: "docs".to_string(),
                    path: "notes/today.md".to_string(),
                    content_hash: "sha256:new".to_string(),
                    kind: FileWriteKind::Delete,
                    approval_id: "approval-1".to_string(),
                },
                1,
            )
            .unwrap_err();
        assert!(
            matches!(err, ConnectorRuntimeError::ReplayWriteQuarantined(operation) if operation == "delete")
        );
    }
}
