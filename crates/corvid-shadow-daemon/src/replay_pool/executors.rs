use super::*;

#[derive(Clone)]
pub struct InterpreterShadowExecutor {
    source_path: PathBuf,
    ir: Arc<IrFile>,
    metadata: Arc<HashMap<String, AgentInvariantInfo>>,
}

#[derive(Clone)]
pub struct NativeShadowExecutor {
    source_path: PathBuf,
    ir: Arc<IrFile>,
    metadata: Arc<HashMap<String, AgentInvariantInfo>>,
    binary_path: PathBuf,
}

impl InterpreterShadowExecutor {
    pub fn from_program_path(path: &Path) -> Result<Self, ShadowExecutorError> {
        if path.extension().and_then(|ext| ext.to_str()) != Some("cor") {
            return Err(ShadowExecutorError::UnsupportedProgramPath(
                path.to_path_buf(),
            ));
        }
        let (ir, metadata) = parse_program_source(path)
            .map_err(|err| ShadowExecutorError::Compile(err.to_string()))?;
        Ok(Self {
            source_path: path.to_path_buf(),
            ir: Arc::new(ir),
            metadata: Arc::new(metadata),
        })
    }

    fn build_runtime(&self, trace_path: &Path, mode: &ShadowExecutionMode) -> Runtime {
        let emit_dir =
            std::env::temp_dir().join(format!("corvid-shadow-{}", corvid_runtime::fresh_run_id()));
        let _ = std::fs::create_dir_all(&emit_dir);
        let tracer = Tracer::open(&emit_dir, corvid_runtime::fresh_run_id())
            .with_redaction(RedactionSet::empty());

        let mut builder = Runtime::builder()
            .tracer(tracer)
            .approver(Arc::new(ProgrammaticApprover::always_yes()))
            .llm(Arc::new(EnvVarMockAdapter::from_env()));

        if let Ok(model) = std::env::var("CORVID_MODEL") {
            builder = builder.default_model(&model);
        }
        if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
            builder = builder.llm(Arc::new(AnthropicAdapter::new(key)));
        }
        if let Ok(key) = std::env::var("OPENAI_API_KEY") {
            builder = builder.llm(Arc::new(OpenAiAdapter::new(key)));
        }

        match mode {
            ShadowExecutionMode::Replay => builder.replay_from(trace_path).build(),
            ShadowExecutionMode::Differential { model } => builder
                .differential_replay_from(trace_path, model.clone())
                .build(),
            ShadowExecutionMode::Mutation(spec) => builder
                .mutation_replay_from(trace_path, spec.step_1based, spec.replacement.clone())
                .build(),
        }
    }
}

impl NativeShadowExecutor {
    pub fn from_program_path(path: &Path) -> Result<Self, ShadowExecutorError> {
        if path.extension().and_then(|ext| ext.to_str()) != Some("cor") {
            return Err(ShadowExecutorError::UnsupportedProgramPath(
                path.to_path_buf(),
            ));
        }
        let source = std::fs::read_to_string(path)
            .map_err(|err| ShadowExecutorError::Io(format!("read `{}`: {err}", path.display())))?;
        let (ir, metadata) = parse_program_source(path)
            .map_err(|err| ShadowExecutorError::Compile(err.to_string()))?;
        let cached = build_or_get_cached_native(path, &source, &ir, None)
            .map_err(|err| ShadowExecutorError::Compile(err.to_string()))?;
        Ok(Self {
            source_path: path.to_path_buf(),
            ir: Arc::new(ir),
            metadata: Arc::new(metadata),
            binary_path: cached.path,
        })
    }
}

#[async_trait]
impl ShadowReplayExecutor for InterpreterShadowExecutor {
    async fn execute(
        &self,
        trace_path: &Path,
        mode: ShadowExecutionMode,
    ) -> Result<ShadowReplayOutcome, ShadowExecutorError> {
        let recorded_events = read_events_from_path(trace_path).map_err(|err| {
            ShadowExecutorError::TraceLoad(format!(
                "failed to load recorded trace `{}`: {err}",
                trace_path.display()
            ))
        })?;
        validate_supported_schema(&recorded_events).map_err(|err| {
            ShadowExecutorError::TraceLoad(format!(
                "unsupported trace schema for `{}`: {err}",
                trace_path.display()
            ))
        })?;

        let writer = recorded_writer(&recorded_events).unwrap_or_default();
        if writer != WRITER_INTERPRETER {
            return Err(ShadowExecutorError::Runtime(
                RuntimeError::CrossTierReplayUnsupported {
                    recorded_writer: writer,
                    replay_writer: WRITER_INTERPRETER.into(),
                },
            ));
        }

        let (agent_name, run_args_json) = recorded_events
            .iter()
            .find_map(|event| match event {
                TraceEvent::RunStarted { agent, args, .. } => Some((agent.clone(), args.clone())),
                _ => None,
            })
            .ok_or_else(|| {
                ShadowExecutorError::TraceLoad(format!(
                    "recorded trace `{}` has no RunStarted event",
                    trace_path.display()
                ))
            })?;

        let recorded_output = recorded_events.iter().find_map(|event| match event {
            TraceEvent::RunCompleted { result, .. } => result.clone(),
            _ => None,
        });
        let run_id = recorded_events
            .iter()
            .find_map(|event| match event {
                TraceEvent::SchemaHeader { run_id, .. }
                | TraceEvent::RunStarted { run_id, .. }
                | TraceEvent::RunCompleted { run_id, .. } => Some(run_id.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "<unknown>".into());

        let agent = self
            .ir
            .agents
            .iter()
            .find(|candidate| candidate.name == agent_name)
            .ok_or_else(|| {
                ShadowExecutorError::Compile(format!(
                    "current program `{}` has no agent named `{agent_name}`",
                    self.source_path.display(),
                ))
            })?;

        let types_by_id = self
            .ir
            .types
            .iter()
            .map(|ty| (ty.id, ty))
            .collect::<HashMap<_, _>>();
        let args = run_args_json
            .iter()
            .zip(agent.params.iter())
            .map(|(json, param)| {
                json_to_value(json.clone(), &param.ty, &types_by_id)
                    .map_err(|err| ShadowExecutorError::Interp(err.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;

        let runtime = self.build_runtime(trace_path, &mode);
        let shadow_trace_path = runtime.tracer().path().to_path_buf();
        let run_result = run_agent(&self.ir, &agent.name, args, &runtime).await;
        let shadow_output = run_result.as_ref().ok().map(value_to_json);
        let error = run_result.as_ref().err().map(|err| err.to_string());
        let ok = run_result.is_ok();
        let replay_divergence = match run_result.as_ref().err().map(|err| &err.kind) {
            Some(corvid_vm::InterpErrorKind::Runtime(RuntimeError::ReplayDivergence(
                divergence,
            ))) => Some(divergence.clone()),
            Some(corvid_vm::InterpErrorKind::Runtime(err)) => {
                return Err(ShadowExecutorError::Runtime(err.clone()));
            }
            Some(other) => return Err(ShadowExecutorError::Interp(other.to_string())),
            None => None,
        };

        let shadow_events = read_events_from_path(&shadow_trace_path).map_err(|err| {
            ShadowExecutorError::TraceLoad(format!(
                "failed to load shadow trace `{}`: {err}",
                shadow_trace_path.display()
            ))
        })?;

        let metadata =
            self.metadata
                .get(&agent.name)
                .cloned()
                .unwrap_or_else(|| AgentInvariantInfo {
                    agent: agent.name.clone(),
                    ..AgentInvariantInfo::default()
                });

        Ok(ShadowReplayOutcome {
            trace_path: trace_path.to_path_buf(),
            run_id,
            agent: agent.name.clone(),
            recorded_dimensions: dimensions_from_trace(&recorded_events, metadata.budget_declared),
            shadow_dimensions: dimensions_from_trace(&shadow_events, metadata.budget_declared),
            recorded_provenance: provenance_from_trace(&recorded_events),
            shadow_provenance: provenance_from_trace(&shadow_events),
            recorded_events,
            shadow_trace_path,
            shadow_events,
            recorded_output,
            shadow_output,
            replay_divergence,
            differential_report: runtime.replay_differential_report(),
            mutation_report: runtime.replay_mutation_report(),
            metadata,
            mode: match mode {
                ShadowExecutionMode::Replay => "replay".into(),
                ShadowExecutionMode::Differential { .. } => "differential".into(),
                ShadowExecutionMode::Mutation(_) => "mutation".into(),
            },
            ok,
            error,
        })
    }
}

#[async_trait]
impl ShadowReplayExecutor for NativeShadowExecutor {
    async fn execute(
        &self,
        trace_path: &Path,
        mode: ShadowExecutionMode,
    ) -> Result<ShadowReplayOutcome, ShadowExecutorError> {
        let recorded_events = read_events_from_path(trace_path).map_err(|err| {
            ShadowExecutorError::TraceLoad(format!(
                "failed to load recorded trace `{}`: {err}",
                trace_path.display()
            ))
        })?;
        validate_supported_schema(&recorded_events).map_err(|err| {
            ShadowExecutorError::TraceLoad(format!(
                "unsupported trace schema for `{}`: {err}",
                trace_path.display()
            ))
        })?;

        let writer = recorded_writer(&recorded_events).unwrap_or_default();
        if writer != WRITER_NATIVE {
            return Err(ShadowExecutorError::Runtime(
                RuntimeError::CrossTierReplayUnsupported {
                    recorded_writer: writer,
                    replay_writer: WRITER_NATIVE.into(),
                },
            ));
        }

        let (agent_name, run_args_json) = recorded_events
            .iter()
            .find_map(|event| match event {
                TraceEvent::RunStarted { agent, args, .. } => Some((agent.clone(), args.clone())),
                _ => None,
            })
            .ok_or_else(|| {
                ShadowExecutorError::TraceLoad(format!(
                    "recorded trace `{}` has no RunStarted event",
                    trace_path.display()
                ))
            })?;

        let recorded_output = recorded_events.iter().find_map(|event| match event {
            TraceEvent::RunCompleted { result, .. } => result.clone(),
            _ => None,
        });
        let run_id = recorded_events
            .iter()
            .find_map(|event| match event {
                TraceEvent::SchemaHeader { run_id, .. }
                | TraceEvent::RunStarted { run_id, .. }
                | TraceEvent::RunCompleted { run_id, .. } => Some(run_id.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "<unknown>".into());

        let agent = self
            .ir
            .agents
            .iter()
            .find(|candidate| candidate.name == agent_name)
            .ok_or_else(|| {
                ShadowExecutorError::Compile(format!(
                    "current program `{}` has no agent named `{agent_name}`",
                    self.source_path.display(),
                ))
            })?;
        let argv = run_args_json
            .iter()
            .zip(agent.params.iter())
            .map(|(json, _param)| native_arg_from_json(json))
            .collect::<Result<Vec<_>, _>>()?;

        let shadow_trace_path = std::env::temp_dir().join(format!(
            "corvid-shadow-native-{}.jsonl",
            corvid_runtime::fresh_run_id()
        ));
        let differential_report_path = std::env::temp_dir().join(format!(
            "corvid-shadow-native-diff-{}.json",
            corvid_runtime::fresh_run_id()
        ));
        let mutation_report_path = std::env::temp_dir().join(format!(
            "corvid-shadow-native-mutation-{}.json",
            corvid_runtime::fresh_run_id()
        ));

        let mut cmd = std::process::Command::new(&self.binary_path);
        cmd.args(&argv)
            .env("CORVID_TRACE_PATH", &shadow_trace_path)
            .env("CORVID_REPLAY_TRACE_PATH", trace_path)
            .env("CORVID_APPROVE_AUTO", "1");
        match &mode {
            ShadowExecutionMode::Replay => {}
            ShadowExecutionMode::Differential { model } => {
                cmd.env("CORVID_REPLAY_MODEL", model).env(
                    "CORVID_REPLAY_DIFFERENTIAL_REPORT_PATH",
                    &differential_report_path,
                );
            }
            ShadowExecutionMode::Mutation(spec) => {
                cmd.env("CORVID_REPLAY_MUTATE_STEP", spec.step_1based.to_string())
                    .env("CORVID_REPLAY_MUTATE_JSON", spec.replacement.to_string())
                    .env("CORVID_REPLAY_MUTATION_REPORT_PATH", &mutation_report_path);
            }
        }

        let output = cmd.output().map_err(|err| {
            ShadowExecutorError::Io(format!(
                "spawn native shadow binary `{}`: {err}",
                self.binary_path.display()
            ))
        })?;
        let ok = output.status.success();
        let error = if ok {
            None
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            Some(if stderr.is_empty() { stdout } else { stderr })
        };

        let shadow_events = read_events_from_path(&shadow_trace_path).map_err(|err| {
            ShadowExecutorError::TraceLoad(format!(
                "failed to load native shadow trace `{}`: {err}",
                shadow_trace_path.display()
            ))
        })?;
        let shadow_output = shadow_events.iter().find_map(|event| match event {
            TraceEvent::RunCompleted { result, .. } => result.clone(),
            _ => None,
        });
        let differential_report = read_optional_json(&differential_report_path)?;
        let mutation_report = read_optional_json(&mutation_report_path)?;

        let metadata =
            self.metadata
                .get(&agent.name)
                .cloned()
                .unwrap_or_else(|| AgentInvariantInfo {
                    agent: agent.name.clone(),
                    ..AgentInvariantInfo::default()
                });

        Ok(ShadowReplayOutcome {
            trace_path: trace_path.to_path_buf(),
            run_id,
            agent: agent.name.clone(),
            recorded_dimensions: dimensions_from_trace(&recorded_events, metadata.budget_declared),
            shadow_dimensions: dimensions_from_trace(&shadow_events, metadata.budget_declared),
            recorded_provenance: provenance_from_trace(&recorded_events),
            shadow_provenance: provenance_from_trace(&shadow_events),
            recorded_events,
            shadow_trace_path,
            shadow_events,
            recorded_output,
            shadow_output,
            replay_divergence: None,
            differential_report,
            mutation_report,
            metadata,
            mode: match mode {
                ShadowExecutionMode::Replay => "replay".into(),
                ShadowExecutionMode::Differential { .. } => "differential".into(),
                ShadowExecutionMode::Mutation(_) => "mutation".into(),
            },
            ok,
            error,
        })
    }
}

fn recorded_writer(events: &[TraceEvent]) -> Option<String> {
    events.iter().find_map(|event| match event {
        TraceEvent::SchemaHeader { writer, .. } => Some(writer.clone()),
        _ => None,
    })
}

fn native_arg_from_json(value: &serde_json::Value) -> Result<String, ShadowExecutorError> {
    match value {
        serde_json::Value::String(s) => Ok(s.clone()),
        serde_json::Value::Number(n) => Ok(n.to_string()),
        serde_json::Value::Bool(b) => Ok(b.to_string()),
        other => Err(ShadowExecutorError::Interp(format!(
            "native shadow replay can pass only scalar CLI arguments today; got `{other}`"
        ))),
    }
}

fn read_optional_json<T>(path: &Path) -> Result<Option<T>, ShadowExecutorError>
where
    T: for<'de> Deserialize<'de>,
{
    match std::fs::read_to_string(path) {
        Ok(raw) if raw.trim().is_empty() => Ok(None),
        Ok(raw) => serde_json::from_str(&raw).map(Some).map_err(|err| {
            ShadowExecutorError::TraceLoad(format!(
                "failed to parse native replay report `{}`: {err}",
                path.display()
            ))
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(ShadowExecutorError::TraceLoad(format!(
            "failed to read native replay report `{}`: {err}",
            path.display()
        ))),
    }
}

fn dimensions_from_trace(events: &[TraceEvent], budget_declared: Option<f64>) -> DimensionSnapshot {
    let mut cost = 0.0;
    let mut latency_ms = 0;
    let mut start = None;
    let mut end = None;
    let mut trust = None;

    for event in events {
        match event {
            TraceEvent::RunStarted { ts_ms, .. } => start = Some(*ts_ms),
            TraceEvent::RunCompleted { ts_ms, .. } => end = Some(*ts_ms),
            TraceEvent::ModelSelected { cost_estimate, .. } => cost += cost_estimate,
            TraceEvent::ApprovalRequest { .. } | TraceEvent::ApprovalResponse { .. } => {
                trust = Some(TrustTier::HumanRequired)
            }
            _ => {}
        }
    }

    if let (Some(start), Some(end)) = (start, end) {
        latency_ms = end.saturating_sub(start);
    }
    if trust.is_none() {
        trust = Some(TrustTier::Autonomous);
    }

    DimensionSnapshot {
        cost,
        latency_ms,
        trust_tier: trust,
        budget_declared,
    }
}

fn provenance_from_trace(events: &[TraceEvent]) -> ProvenanceSnapshot {
    let mut nodes = BTreeSet::new();
    let mut roots = BTreeSet::new();
    let mut has_chain = false;
    for event in events {
        if let TraceEvent::ProvenanceEdge {
            node_id,
            parents,
            op,
            ..
        } = event
        {
            nodes.insert(node_id.clone());
            if parents.is_empty() {
                roots.insert(op.clone());
            }
            has_chain = true;
        }
    }
    ProvenanceSnapshot {
        nodes,
        root_sources: roots,
        has_chain,
    }
}
