use super::{Runtime, RuntimeMode};
use crate::approvals::{Approver, StdinApprover};
use crate::calibration::CalibrationStore;
use crate::errors::RuntimeError;
use crate::http::HttpClient;
use crate::human::{HumanInteractor, StdinHumanInteractor};
use crate::io::IoRuntime;
use crate::llm::{LlmAdapter, LlmRegistry};
use crate::models::{ModelCatalog, RegisteredModel};
use crate::prompt_cache::PromptCache;
use crate::queue::QueueRuntime;
use crate::record::Recorder;
use crate::replay::ReplaySource;
use crate::secrets::SecretRuntime;
use crate::store::StoreManager;
use crate::tools::ToolRegistry;
use crate::tracing::{fresh_run_id, Tracer};
use crate::usage::LlmUsageLedger;
use corvid_trace_schema::WRITER_INTERPRETER;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
pub struct RuntimeBuilder {
    tools: ToolRegistry,
    llms: LlmRegistry,
    approver: Option<Arc<dyn Approver>>,
    human: Option<Arc<dyn HumanInteractor>>,
    tracer: Option<Tracer>,
    pub(super) trace_schema_writer: &'static str,
    default_model: String,
    model_catalog: ModelCatalog,
    model_catalog_root: Option<PathBuf>,
    rollout_seed: Option<u64>,
    pub(super) replay_trace: Option<PathBuf>,
    pub(super) replay_model_swap: Option<String>,
    replay_mutation: Option<(usize, serde_json::Value)>,
    stores: StoreManager,
}

impl Default for RuntimeBuilder {
    fn default() -> Self {
        Self {
            tools: ToolRegistry::default(),
            llms: LlmRegistry::default(),
            approver: None,
            human: None,
            tracer: None,
            trace_schema_writer: WRITER_INTERPRETER,
            default_model: String::new(),
            model_catalog: ModelCatalog::default(),
            model_catalog_root: None,
            rollout_seed: None,
            replay_trace: None,
            replay_model_swap: None,
            replay_mutation: None,
            stores: StoreManager::default(),
        }
    }
}

impl RuntimeBuilder {
    pub fn tool<F, Fut>(mut self, name: impl Into<String>, handler: F) -> Self
    where
        F: Fn(Vec<serde_json::Value>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<serde_json::Value, RuntimeError>> + Send + 'static,
    {
        self.tools.register(name, handler);
        self
    }

    /// Register deterministic mock tool handlers from
    /// `CORVID_TEST_MOCK_TOOLS`.
    ///
    /// The env var is a JSON object whose keys are tool names. Each value may
    /// be either a single JSON response or an array of responses consumed in
    /// FIFO order by that tool.
    pub fn env_mock_tools_from_env(mut self) -> Self {
        let Some(map) = std::env::var("CORVID_TEST_MOCK_TOOLS")
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&raw).ok())
        else {
            return self;
        };

        self.register_mock_tool_responses(map);
        self
    }

    fn register_mock_tool_responses(&mut self, map: serde_json::Map<String, serde_json::Value>) {
        for (name, value) in map {
            let queue = match value {
                serde_json::Value::Array(values) => values.into_iter().collect(),
                other => VecDeque::from([other]),
            };
            let responses = Arc::new(Mutex::new(queue));
            let tool_name = name.clone();
            self.tools.register(name, move |_| {
                let responses = Arc::clone(&responses);
                let tool_name = tool_name.clone();
                async move {
                    responses
                        .lock()
                        .unwrap()
                        .pop_front()
                        .ok_or_else(|| RuntimeError::ToolFailed {
                            tool: tool_name,
                            message: "CORVID_TEST_MOCK_TOOLS response queue exhausted".into(),
                        })
                }
            });
        }
    }

    pub fn llm(mut self, adapter: Arc<dyn LlmAdapter>) -> Self {
        self.llms.register(adapter);
        self
    }

    pub fn approver(mut self, approver: Arc<dyn Approver>) -> Self {
        self.approver = Some(approver);
        self
    }

    pub fn human_interactor(mut self, human: Arc<dyn HumanInteractor>) -> Self {
        self.human = Some(human);
        self
    }

    pub fn tracer(mut self, tracer: Tracer) -> Self {
        self.tracer = Some(tracer);
        self
    }

    pub fn trace_schema_writer(mut self, writer: &'static str) -> Self {
        self.trace_schema_writer = writer;
        self
    }

    /// Open a JSONL trace file under `dir` with a fresh run id.
    pub fn trace_to(self, dir: &Path) -> Self {
        let tracer = Tracer::open(dir, fresh_run_id());
        self.tracer(tracer)
    }

    pub fn default_model(mut self, model: impl Into<String>) -> Self {
        self.default_model = model.into();
        self
    }

    pub fn model(mut self, model: RegisteredModel) -> Self {
        self.model_catalog.register(model);
        self
    }

    pub fn model_catalog(mut self, catalog: ModelCatalog) -> Self {
        self.model_catalog = catalog;
        self
    }

    pub fn model_catalog_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.model_catalog_root = Some(root.into());
        self
    }

    pub fn stores(mut self, stores: StoreManager) -> Self {
        self.stores = stores;
        self
    }

    pub fn sqlite_store(mut self, path: impl AsRef<Path>) -> Result<Self, RuntimeError> {
        self.stores = StoreManager::sqlite(path)?;
        Ok(self)
    }

    pub fn rollout_seed(mut self, seed: u64) -> Self {
        self.rollout_seed = Some(seed);
        self
    }

    pub fn replay_from(mut self, path: impl Into<PathBuf>) -> Self {
        self.replay_trace = Some(path.into());
        self
    }

    pub fn replay_model_swap(mut self, model: impl Into<String>) -> Self {
        self.replay_model_swap = Some(model.into());
        self
    }

    pub fn differential_replay_from(
        mut self,
        path: impl Into<PathBuf>,
        model: impl Into<String>,
    ) -> Self {
        self.replay_trace = Some(path.into());
        self.replay_model_swap = Some(model.into());
        self
    }

    pub fn mutation_replay_from(
        mut self,
        path: impl Into<PathBuf>,
        step_1based: usize,
        replacement: serde_json::Value,
    ) -> Self {
        self.replay_trace = Some(path.into());
        self.replay_mutation = Some((step_1based, replacement));
        self
    }

    pub fn build(self) -> Runtime {
        let mut model_catalog = self.model_catalog;
        let model_catalog_error = if model_catalog.is_empty() {
            let start = self
                .model_catalog_root
                .or_else(|| std::env::current_dir().ok());
            match start {
                Some(start) => match ModelCatalog::load_walking(&start) {
                    Ok(Some(loaded)) => {
                        model_catalog.extend(loaded);
                        None
                    }
                    Ok(None) => None,
                    Err(err) => Some(err),
                },
                None => None,
            }
        } else {
            None
        };
        let tracer = self.tracer.unwrap_or_else(Tracer::null);
        let recorder = Recorder::for_tracer(&tracer, self.trace_schema_writer).map(Arc::new);
        let (mode, replay_error, rollout_seed) = if let Some(path) = self.replay_trace {
            let replay_load = if let Some((step_1based, replacement)) = self.replay_mutation {
                ReplaySource::from_path_for_writer_with_mutation(
                    path,
                    self.trace_schema_writer,
                    step_1based,
                    replacement,
                )
            } else if let Some(model) = self.replay_model_swap {
                ReplaySource::from_path_for_writer_with_model(path, self.trace_schema_writer, model)
            } else {
                ReplaySource::from_path_for_writer(path, self.trace_schema_writer)
            };
            match replay_load {
                Ok(source) => (
                    RuntimeMode::Replay(source.clone()),
                    None,
                    source.initial_rollout_seed(),
                ),
                Err(err) => (
                    RuntimeMode::Live,
                    Some(err),
                    self.rollout_seed.unwrap_or_else(crate::tracing::now_ms),
                ),
            }
        } else {
            (
                RuntimeMode::Live,
                None,
                self.rollout_seed.unwrap_or_else(crate::tracing::now_ms),
            )
        };
        if let Some(recorder) = &recorder {
            recorder.emit_schema_header();
            recorder.emit_seed_read("rollout_default_seed", rollout_seed);
        }
        Runtime {
            tools: self.tools,
            llms: self.llms,
            approver: self
                .approver
                .unwrap_or_else(|| Arc::new(StdinApprover::new())),
            human: self
                .human
                .unwrap_or_else(|| Arc::new(StdinHumanInteractor::new())),
            tracer,
            recorder,
            mode,
            replay_error,
            default_model: self.default_model,
            model_catalog,
            model_catalog_error,
            rollout_state: Arc::new(AtomicU64::new(rollout_seed)),
            calibration: CalibrationStore::default(),
            prompt_cache: PromptCache::default(),
            stores: self.stores,
            usage_ledger: LlmUsageLedger::new(),
            http: HttpClient::new(),
            io: IoRuntime::new(),
            secrets: SecretRuntime::new(),
            queue: QueueRuntime::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn mock_tool_responses_are_registered_and_consumed_fifo() {
        let mut map = serde_json::Map::new();
        map.insert("lookup".to_string(), json!(["first", "second"]));

        let mut builder = Runtime::builder();
        builder.register_mock_tool_responses(map);
        let runtime = builder.build();

        let first = runtime.tools().call("lookup", vec![]).await.unwrap();
        let second = runtime.tools().call("lookup", vec![]).await.unwrap();
        assert_eq!(first, json!("first"));
        assert_eq!(second, json!("second"));
    }

    #[tokio::test]
    async fn mock_tool_response_queue_exhaustion_is_explicit() {
        let mut map = serde_json::Map::new();
        map.insert("lookup".to_string(), json!("only"));

        let mut builder = Runtime::builder();
        builder.register_mock_tool_responses(map);
        let runtime = builder.build();

        runtime.tools().call("lookup", vec![]).await.unwrap();
        let err = runtime.tools().call("lookup", vec![]).await.unwrap_err();
        assert!(matches!(
            err,
            RuntimeError::ToolFailed { ref tool, ref message }
                if tool == "lookup" && message.contains("response queue exhausted")
        ));
    }
}
