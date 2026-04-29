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
            ["tasks.linear_search", "tasks.github_search"],
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
}
