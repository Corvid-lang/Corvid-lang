use crate::{
    parse_connector_manifest, ConnectorAuthState, ConnectorManifest, ConnectorRequest,
    ConnectorRuntime, ConnectorRuntimeError, ConnectorRuntimeMode,
};
use serde::{Deserialize, Serialize};

pub const TASK_CONNECTOR_MANIFEST: &str = r#"
schema = "corvid.connector.v1"
name = "tasks"
provider = "linear_github"
mode = ["mock", "replay", "real"]

[[scope]]
id = "tasks.linear_search"
provider_scope = "linear:read"
data_classes = ["task_metadata"]
effects = ["network.read"]
approval = "none"

[[scope]]
id = "tasks.github_search"
provider_scope = "github:issues:read"
data_classes = ["task_metadata"]
effects = ["network.read"]
approval = "none"

[[scope]]
id = "tasks.linear_write"
provider_scope = "linear:write"
data_classes = ["task_metadata", "task_body"]
effects = ["network.write", "task.write"]
approval = "required"

[[scope]]
id = "tasks.github_write"
provider_scope = "github:issues:write"
data_classes = ["task_metadata", "task_body"]
effects = ["network.write", "task.write"]
approval = "required"

[[rate_limit]]
key = "tenant_user"
limit = 100
window_ms = 1000
retry_after = "provider_header"

[[redaction]]
field = "issue.body"
strategy = "hash_and_drop"

[[replay]]
operation = "linear_search"
policy = "record_read"

[[replay]]
operation = "github_search"
policy = "record_read"

[[replay]]
operation = "linear_write"
policy = "quarantine_write"

[[replay]]
operation = "github_write"
policy = "quarantine_write"
"#;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskIssue {
    pub provider: String,
    pub id: String,
    pub key: String,
    pub title: String,
    pub state: String,
    pub assignee: String,
    pub updated_ms: u64,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinearIssueSearchRequest {
    pub workspace_id: String,
    pub query: String,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHubIssueSearchRequest {
    pub owner: String,
    pub repo: String,
    pub query: String,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskWriteKind {
    Create,
    Update,
    Comment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskWriteRequest {
    pub provider: String,
    pub workspace_or_repo: String,
    pub issue_id: Option<String>,
    pub title: String,
    pub body: String,
    pub kind: TaskWriteKind,
    pub approval_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskWriteReceipt {
    pub provider: String,
    pub id: String,
    pub key: String,
    pub approval_id: String,
    pub replay_key: String,
}

#[derive(Debug, Clone)]
pub struct TaskConnector {
    runtime: ConnectorRuntime,
}

impl TaskConnector {
    pub fn new(
        auth: ConnectorAuthState,
        mode: ConnectorRuntimeMode,
    ) -> Result<Self, toml::de::Error> {
        Ok(Self {
            runtime: ConnectorRuntime::new(task_manifest()?, auth, mode),
        })
    }

    pub fn insert_mock(&mut self, operation: impl Into<String>, payload: serde_json::Value) {
        self.runtime.insert_mock(operation, payload);
    }

    pub fn search_linear(
        &mut self,
        request: LinearIssueSearchRequest,
        now_ms: u64,
    ) -> Result<Vec<TaskIssue>, ConnectorRuntimeError> {
        let replay_key = format!(
            "tasks:linear:{}:{}",
            request.workspace_id,
            stable(&request.query)
        );
        let response = self.runtime.execute(ConnectorRequest {
            scope_id: "tasks.linear_search".to_string(),
            operation: "linear_search".to_string(),
            payload: serde_json::to_value(&request).unwrap_or_default(),
            approval_id: String::new(),
            replay_key,
            now_ms,
        })?;
        Ok(serde_json::from_value(response.payload).unwrap_or_default())
    }

    pub fn search_github(
        &mut self,
        request: GitHubIssueSearchRequest,
        now_ms: u64,
    ) -> Result<Vec<TaskIssue>, ConnectorRuntimeError> {
        let replay_key = format!(
            "tasks:github:{}/{}:{}",
            request.owner,
            request.repo,
            stable(&request.query)
        );
        let response = self.runtime.execute(ConnectorRequest {
            scope_id: "tasks.github_search".to_string(),
            operation: "github_search".to_string(),
            payload: serde_json::to_value(&request).unwrap_or_default(),
            approval_id: String::new(),
            replay_key,
            now_ms,
        })?;
        Ok(serde_json::from_value(response.payload).unwrap_or_default())
    }

    pub fn write_linear(
        &mut self,
        request: TaskWriteRequest,
        now_ms: u64,
    ) -> Result<TaskWriteReceipt, ConnectorRuntimeError> {
        self.write_task("tasks.linear_write", "linear_write", request, now_ms)
    }

    pub fn write_github(
        &mut self,
        request: TaskWriteRequest,
        now_ms: u64,
    ) -> Result<TaskWriteReceipt, ConnectorRuntimeError> {
        self.write_task("tasks.github_write", "github_write", request, now_ms)
    }

    fn write_task(
        &mut self,
        scope_id: &str,
        operation: &str,
        request: TaskWriteRequest,
        now_ms: u64,
    ) -> Result<TaskWriteReceipt, ConnectorRuntimeError> {
        let replay_key = format!(
            "tasks:{}:{}:{}:{:?}",
            operation,
            request.workspace_or_repo,
            request
                .issue_id
                .clone()
                .unwrap_or_else(|| "new".to_string()),
            request.kind
        );
        let approval_id = request.approval_id.clone();
        let response = self.runtime.execute(ConnectorRequest {
            scope_id: scope_id.to_string(),
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

pub fn task_manifest() -> Result<ConnectorManifest, toml::de::Error> {
    parse_connector_manifest(TASK_CONNECTOR_MANIFEST)
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
            [
                "tasks.linear_search",
                "tasks.github_search",
                "tasks.linear_write",
                "tasks.github_write",
            ],
            10_000,
        )
    }

    #[test]
    fn task_manifest_validates_read_contract() {
        let manifest = task_manifest().unwrap();
        let report = validate_connector_manifest(&manifest);
        assert!(report.valid, "{report:?}");
    }

    #[test]
    fn linear_and_github_search_work_in_mock_mode() {
        let mut connector = TaskConnector::new(auth(), ConnectorRuntimeMode::Mock).unwrap();
        let linear = TaskIssue {
            provider: "linear".to_string(),
            id: "lin-1".to_string(),
            key: "COR-1".to_string(),
            title: "Triage inbox".to_string(),
            state: "Todo".to_string(),
            assignee: "alice".to_string(),
            updated_ms: 100,
            url: "https://linear.app/corvid/issue/COR-1".to_string(),
        };
        let github = TaskIssue {
            provider: "github".to_string(),
            id: "gh-1".to_string(),
            key: "#42".to_string(),
            title: "Fix connector".to_string(),
            state: "open".to_string(),
            assignee: "bob".to_string(),
            updated_ms: 200,
            url: "https://github.com/corvid-lang/corvid/issues/42".to_string(),
        };
        connector.insert_mock("linear_search", serde_json::json!([linear.clone()]));
        connector.insert_mock("github_search", serde_json::json!([github.clone()]));

        assert_eq!(
            connector
                .search_linear(
                    LinearIssueSearchRequest {
                        workspace_id: "corvid".to_string(),
                        query: "state:todo".to_string(),
                        limit: 10,
                    },
                    1,
                )
                .unwrap(),
            vec![linear]
        );
        assert_eq!(
            connector
                .search_github(
                    GitHubIssueSearchRequest {
                        owner: "corvid-lang".to_string(),
                        repo: "corvid".to_string(),
                        query: "is:open".to_string(),
                        limit: 10,
                    },
                    2,
                )
                .unwrap(),
            vec![github]
        );
    }

    #[test]
    fn linear_and_github_writes_require_approval_and_work_in_mock_mode() {
        let mut connector = TaskConnector::new(auth(), ConnectorRuntimeMode::Mock).unwrap();
        connector.insert_mock(
            "linear_write",
            serde_json::json!({
                "provider": "linear",
                "id": "lin-2",
                "key": "COR-2",
                "approval_id": "approval-1",
                "replay_key": "tasks:linear_write:corvid:new:Create"
            }),
        );
        connector.insert_mock(
            "github_write",
            serde_json::json!({
                "provider": "github",
                "id": "gh-2",
                "key": "#43",
                "approval_id": "approval-1",
                "replay_key": "tasks:github_write:corvid-lang/corvid:new:Create"
            }),
        );

        let missing = connector
            .write_linear(
                TaskWriteRequest {
                    provider: "linear".to_string(),
                    workspace_or_repo: "corvid".to_string(),
                    issue_id: None,
                    title: "Build connector".to_string(),
                    body: "details".to_string(),
                    kind: TaskWriteKind::Create,
                    approval_id: String::new(),
                },
                1,
            )
            .unwrap_err();
        assert!(
            matches!(missing, ConnectorRuntimeError::ApprovalRequired(scope) if scope == "tasks.linear_write")
        );

        let linear = connector
            .write_linear(
                TaskWriteRequest {
                    provider: "linear".to_string(),
                    workspace_or_repo: "corvid".to_string(),
                    issue_id: None,
                    title: "Build connector".to_string(),
                    body: "details".to_string(),
                    kind: TaskWriteKind::Create,
                    approval_id: "approval-1".to_string(),
                },
                2,
            )
            .unwrap();
        assert_eq!(linear.key, "COR-2");

        let github = connector
            .write_github(
                TaskWriteRequest {
                    provider: "github".to_string(),
                    workspace_or_repo: "corvid-lang/corvid".to_string(),
                    issue_id: None,
                    title: "Build connector".to_string(),
                    body: "details".to_string(),
                    kind: TaskWriteKind::Create,
                    approval_id: "approval-1".to_string(),
                },
                3,
            )
            .unwrap();
        assert_eq!(github.key, "#43");
    }

    #[test]
    fn task_replay_quarantines_writes() {
        let mut connector = TaskConnector::new(auth(), ConnectorRuntimeMode::Replay).unwrap();
        let err = connector
            .write_github(
                TaskWriteRequest {
                    provider: "github".to_string(),
                    workspace_or_repo: "corvid-lang/corvid".to_string(),
                    issue_id: Some("42".to_string()),
                    title: "Update".to_string(),
                    body: "comment".to_string(),
                    kind: TaskWriteKind::Comment,
                    approval_id: "approval-1".to_string(),
                },
                1,
            )
            .unwrap_err();
        assert!(
            matches!(err, ConnectorRuntimeError::ReplayWriteQuarantined(operation) if operation == "github_write")
        );
    }
}
