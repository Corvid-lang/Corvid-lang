use crate::errors::RuntimeError;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub struct RegisteredModel {
    pub name: String,
    pub provider: Option<String>,
    pub capability: Option<String>,
    pub version: Option<String>,
    pub output_format: Option<String>,
    pub privacy_tier: Option<String>,
    pub jurisdiction: Option<String>,
    pub latency_tier: Option<String>,
    pub context_window: Option<u64>,
    pub structured_output: bool,
    pub tool_calling: bool,
    pub embeddings: bool,
    pub multimodal: Vec<String>,
    pub task_capabilities: Vec<String>,
    pub cost_per_token_in: f64,
    pub cost_per_token_out: f64,
}

impl RegisteredModel {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            provider: None,
            capability: None,
            version: None,
            output_format: None,
            privacy_tier: None,
            jurisdiction: None,
            latency_tier: None,
            context_window: None,
            structured_output: false,
            tool_calling: false,
            embeddings: false,
            multimodal: Vec::new(),
            task_capabilities: Vec::new(),
            cost_per_token_in: 0.0,
            cost_per_token_out: 0.0,
        }
    }

    pub fn provider(mut self, provider: impl Into<String>) -> Self {
        self.provider = Some(provider.into());
        self
    }

    pub fn capability(mut self, capability: impl Into<String>) -> Self {
        self.capability = Some(capability.into());
        self
    }

    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    pub fn output_format(mut self, output_format: impl Into<String>) -> Self {
        self.output_format = Some(output_format.into());
        self
    }

    pub fn privacy_tier(mut self, privacy_tier: impl Into<String>) -> Self {
        self.privacy_tier = Some(privacy_tier.into());
        self
    }

    pub fn jurisdiction(mut self, jurisdiction: impl Into<String>) -> Self {
        self.jurisdiction = Some(jurisdiction.into());
        self
    }

    pub fn latency_tier(mut self, latency_tier: impl Into<String>) -> Self {
        self.latency_tier = Some(latency_tier.into());
        self
    }

    pub fn context_window(mut self, context_window: u64) -> Self {
        self.context_window = Some(context_window);
        self
    }

    pub fn structured_output(mut self, supported: bool) -> Self {
        self.structured_output = supported;
        self
    }

    pub fn tool_calling(mut self, supported: bool) -> Self {
        self.tool_calling = supported;
        self
    }

    pub fn embeddings(mut self, supported: bool) -> Self {
        self.embeddings = supported;
        self
    }

    pub fn multimodal(mut self, tags: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.multimodal = tags.into_iter().map(Into::into).collect();
        self
    }

    pub fn task_capabilities(mut self, tags: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.task_capabilities = tags.into_iter().map(Into::into).collect();
        self
    }

    pub fn cost_per_token_in(mut self, cost: f64) -> Self {
        self.cost_per_token_in = cost;
        self
    }

    pub fn cost_per_token_out(mut self, cost: f64) -> Self {
        self.cost_per_token_out = cost;
        self
    }

    pub fn estimated_cost(&self, prompt_tokens: u64, completion_tokens: u64) -> f64 {
        self.cost_per_token_in * prompt_tokens as f64
            + self.cost_per_token_out * completion_tokens as f64
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelCatalog {
    models: BTreeMap<String, RegisteredModel>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelSelection {
    pub model: String,
    pub provider: Option<String>,
    pub capability_required: Option<String>,
    pub capability_picked: Option<String>,
    pub version: Option<String>,
    pub output_format_required: Option<String>,
    pub output_format_picked: Option<String>,
    pub privacy_tier: Option<String>,
    pub jurisdiction: Option<String>,
    pub latency_tier: Option<String>,
    pub context_window: Option<u64>,
    pub structured_output: bool,
    pub tool_calling: bool,
    pub embeddings: bool,
    pub multimodal: Vec<String>,
    pub task_capabilities: Vec<String>,
    pub cost_estimate: f64,
}

impl ModelCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, model: RegisteredModel) {
        self.models.insert(model.name.clone(), model);
    }

    pub fn extend(&mut self, other: Self) {
        for model in other.models.into_values() {
            self.register(model);
        }
    }

    pub fn get(&self, name: &str) -> Option<&RegisteredModel> {
        self.models.get(name)
    }

    pub fn names(&self) -> Vec<String> {
        self.models.keys().cloned().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    pub fn load_from_path(path: &Path) -> Result<Option<Self>, RuntimeError> {
        let body = match std::fs::read_to_string(path) {
            Ok(body) => body,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => {
                return Err(RuntimeError::ModelCatalogParse {
                    path: path.to_path_buf(),
                    message: err.to_string(),
                })
            }
        };
        let value = toml::from_str::<toml::Value>(&body).map_err(|err| RuntimeError::ModelCatalogParse {
            path: path.to_path_buf(),
            message: err.to_string(),
        })?;
        parse_catalog_toml(path, value).map(Some)
    }

    pub fn load_walking(start: &Path) -> Result<Option<Self>, RuntimeError> {
        let mut cur = Some(start);
        while let Some(dir) = cur {
            let candidate = dir.join("corvid.toml");
            if candidate.exists() {
                return Self::load_from_path(&candidate);
            }
            cur = dir.parent();
        }
        Ok(None)
    }

    pub fn select_cheapest_by_capability(
        &self,
        required_capability: &str,
        prompt_tokens: u64,
        completion_tokens: u64,
    ) -> Result<ModelSelection, RuntimeError> {
        self.select_cheapest_by_requirements(
            Some(required_capability),
            None,
            prompt_tokens,
            completion_tokens,
        )
    }

    pub fn select_cheapest_by_requirements(
        &self,
        required_capability: Option<&str>,
        required_output_format: Option<&str>,
        prompt_tokens: u64,
        completion_tokens: u64,
    ) -> Result<ModelSelection, RuntimeError> {
        let available_models = self.names();
        let selected = self
            .models
            .values()
            .filter(|model| {
                required_capability
                    .map(|required| capability_satisfies(model.capability.as_deref(), required))
                    .unwrap_or(true)
            })
            .filter(|model| {
                required_output_format
                    .map(|required| model.output_format.as_deref() == Some(required))
                    .unwrap_or(true)
            })
            .min_by(|left, right| {
                let left_cost = left.estimated_cost(prompt_tokens, completion_tokens);
                let right_cost = right.estimated_cost(prompt_tokens, completion_tokens);
                left_cost
                    .partial_cmp(&right_cost)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| left.name.cmp(&right.name))
            })
            .ok_or_else(|| RuntimeError::NoEligibleModel {
                required_capability: required_capability.unwrap_or("any").to_string(),
                required_output_format: required_output_format.map(ToString::to_string),
                available_models,
            })?;

        Ok(ModelSelection {
            model: selected.name.clone(),
            provider: selected.provider.clone(),
            capability_required: required_capability.map(ToString::to_string),
            capability_picked: selected.capability.clone(),
            version: selected.version.clone(),
            output_format_required: required_output_format.map(ToString::to_string),
            output_format_picked: selected.output_format.clone(),
            privacy_tier: selected.privacy_tier.clone(),
            jurisdiction: selected.jurisdiction.clone(),
            latency_tier: selected.latency_tier.clone(),
            context_window: selected.context_window,
            structured_output: selected.structured_output,
            tool_calling: selected.tool_calling,
            embeddings: selected.embeddings,
            multimodal: selected.multimodal.clone(),
            task_capabilities: selected.task_capabilities.clone(),
            cost_estimate: selected.estimated_cost(prompt_tokens, completion_tokens),
        })
    }

    pub fn describe_named_model(
        &self,
        model_name: &str,
        prompt_tokens: u64,
        completion_tokens: u64,
    ) -> ModelSelection {
        match self.get(model_name) {
            Some(model) => ModelSelection {
                model: model.name.clone(),
                provider: model.provider.clone(),
                capability_required: None,
                capability_picked: model.capability.clone(),
                version: model.version.clone(),
                output_format_required: None,
                output_format_picked: model.output_format.clone(),
                privacy_tier: model.privacy_tier.clone(),
                jurisdiction: model.jurisdiction.clone(),
                latency_tier: model.latency_tier.clone(),
                context_window: model.context_window,
                structured_output: model.structured_output,
                tool_calling: model.tool_calling,
                embeddings: model.embeddings,
                multimodal: model.multimodal.clone(),
                task_capabilities: model.task_capabilities.clone(),
                cost_estimate: model.estimated_cost(prompt_tokens, completion_tokens),
            },
            None => ModelSelection {
                model: model_name.to_string(),
                provider: None,
                capability_required: None,
                capability_picked: None,
                version: None,
                output_format_required: None,
                output_format_picked: None,
                privacy_tier: None,
                jurisdiction: None,
                latency_tier: None,
                context_window: None,
                structured_output: false,
                tool_calling: false,
                embeddings: false,
                multimodal: Vec::new(),
                task_capabilities: Vec::new(),
                cost_estimate: 0.0,
            },
        }
    }

    pub fn compatible_fallbacks_for(
        &self,
        primary_model: &str,
        prompt_tokens: u64,
        completion_tokens: u64,
    ) -> Vec<ModelSelection> {
        let Some(primary) = self.get(primary_model) else {
            return Vec::new();
        };
        let mut candidates: Vec<_> = self
            .models
            .values()
            .filter(|candidate| candidate.name != primary.name)
            .filter(|candidate| model_is_compatible_fallback(primary, candidate))
            .map(|candidate| {
                self.describe_named_model(&candidate.name, prompt_tokens, completion_tokens)
            })
            .collect();
        candidates.sort_by(|left, right| {
            left.cost_estimate
                .partial_cmp(&right.cost_estimate)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.model.cmp(&right.model))
        });
        candidates
    }
}

fn parse_catalog_toml(path: &Path, value: toml::Value) -> Result<ModelCatalog, RuntimeError> {
    let Some(root) = value.as_table() else {
        return Ok(ModelCatalog::new());
    };
    let Some(llm) = root.get("llm").and_then(toml::Value::as_table) else {
        return Ok(ModelCatalog::new());
    };
    let Some(models) = llm.get("models").and_then(toml::Value::as_table) else {
        return Ok(ModelCatalog::new());
    };

    let mut catalog = ModelCatalog::new();
    for (name, spec) in models {
        let Some(spec) = spec.as_table() else {
            return Err(RuntimeError::ModelCatalogParse {
                path: path.to_path_buf(),
                message: format!("`[llm.models.{name}]` must be a table"),
            });
        };
        let capability = spec
            .get("capability")
            .and_then(toml::Value::as_str)
            .map(ToString::to_string);
        let provider = spec
            .get("provider")
            .and_then(toml::Value::as_str)
            .map(ToString::to_string);
        let version = spec
            .get("version")
            .and_then(toml::Value::as_str)
            .map(ToString::to_string);
        let output_format = spec
            .get("output_format")
            .and_then(toml::Value::as_str)
            .map(ToString::to_string);
        let privacy_tier = spec
            .get("privacy_tier")
            .and_then(toml::Value::as_str)
            .map(ToString::to_string);
        let jurisdiction = spec
            .get("jurisdiction")
            .and_then(toml::Value::as_str)
            .map(ToString::to_string);
        let latency_tier = spec
            .get("latency_tier")
            .and_then(toml::Value::as_str)
            .map(ToString::to_string);
        let context_window =
            parse_optional_u64_field(path, name, "context_window", spec.get("context_window"))?;
        let structured_output = parse_bool_field(spec.get("structured_output"));
        let tool_calling = parse_bool_field(spec.get("tool_calling"));
        let embeddings = parse_bool_field(spec.get("embeddings"));
        let multimodal = parse_string_list_field(path, name, "multimodal", spec.get("multimodal"))?;
        let task_capabilities = parse_string_list_field(
            path,
            name,
            "task_capabilities",
            spec.get("task_capabilities"),
        )?;
        let cost_per_token_in = parse_cost_field(path, name, spec.get("cost_per_token_in"))?;
        let cost_per_token_out = parse_cost_field(path, name, spec.get("cost_per_token_out"))?;
        catalog.register(RegisteredModel {
            name: name.clone(),
            provider,
            capability,
            version,
            output_format,
            privacy_tier,
            jurisdiction,
            latency_tier,
            context_window,
            structured_output,
            tool_calling,
            embeddings,
            multimodal,
            task_capabilities,
            cost_per_token_in,
            cost_per_token_out,
        });
    }
    Ok(catalog)
}

fn parse_optional_u64_field(
    path: &Path,
    model_name: &str,
    field: &str,
    value: Option<&toml::Value>,
) -> Result<Option<u64>, RuntimeError> {
    let Some(value) = value else {
        return Ok(None);
    };
    match value {
        toml::Value::Integer(value) if *value >= 0 => Ok(Some(*value as u64)),
        other => Err(RuntimeError::ModelCatalogParse {
            path: path.to_path_buf(),
            message: format!(
                "model `{model_name}` field `{field}` must be a non-negative integer, got `{other}`"
            ),
        }),
    }
}

fn parse_bool_field(value: Option<&toml::Value>) -> bool {
    value.and_then(toml::Value::as_bool).unwrap_or(false)
}

fn parse_string_list_field(
    path: &Path,
    model_name: &str,
    field: &str,
    value: Option<&toml::Value>,
) -> Result<Vec<String>, RuntimeError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let Some(values) = value.as_array() else {
        return Err(RuntimeError::ModelCatalogParse {
            path: path.to_path_buf(),
            message: format!("model `{model_name}` field `{field}` must be a string list"),
        });
    };
    values
        .iter()
        .map(|value| {
            value.as_str().map(ToString::to_string).ok_or_else(|| {
                RuntimeError::ModelCatalogParse {
                    path: path.to_path_buf(),
                    message: format!(
                        "model `{model_name}` field `{field}` must contain only strings"
                    ),
                }
            })
        })
        .collect()
}

fn parse_cost_field(
    path: &Path,
    model_name: &str,
    value: Option<&toml::Value>,
) -> Result<f64, RuntimeError> {
    let Some(value) = value else {
        return Ok(0.0);
    };
    match value {
        toml::Value::Float(value) => Ok(*value),
        toml::Value::Integer(value) => Ok(*value as f64),
        toml::Value::String(value) => parse_cost_string(value).ok_or_else(|| RuntimeError::ModelCatalogParse {
            path: path.to_path_buf(),
            message: format!(
                "model `{model_name}` field must be a number or `$...` string, got `{value}`"
            ),
        }),
        other => Err(RuntimeError::ModelCatalogParse {
            path: path.to_path_buf(),
            message: format!("unsupported cost value for model `{model_name}`: {other}"),
        }),
    }
}

fn parse_cost_string(value: &str) -> Option<f64> {
    value
        .strip_prefix('$')
        .unwrap_or(value)
        .parse::<f64>()
        .ok()
}

fn capability_satisfies(model_capability: Option<&str>, required: &str) -> bool {
    let Some(model_capability) = model_capability else {
        return false;
    };
    match (capability_rank(model_capability), capability_rank(required)) {
        (Some(model_rank), Some(required_rank)) => model_rank >= required_rank,
        _ => model_capability == required,
    }
}

fn model_is_compatible_fallback(primary: &RegisteredModel, candidate: &RegisteredModel) -> bool {
    if primary.provider.is_some() && primary.provider == candidate.provider {
        return false;
    }
    if let Some(required) = primary.capability.as_deref() {
        if !capability_satisfies(candidate.capability.as_deref(), required) {
            return false;
        }
    }
    if primary.output_format.is_some() && primary.output_format != candidate.output_format {
        return false;
    }
    if primary.privacy_tier.is_some() && primary.privacy_tier != candidate.privacy_tier {
        return false;
    }
    if primary.jurisdiction.is_some() && primary.jurisdiction != candidate.jurisdiction {
        return false;
    }
    if primary.structured_output && !candidate.structured_output {
        return false;
    }
    if primary.tool_calling && !candidate.tool_calling {
        return false;
    }
    if primary.embeddings && !candidate.embeddings {
        return false;
    }
    if let Some(required_context) = primary.context_window {
        if candidate.context_window.unwrap_or(0) < required_context {
            return false;
        }
    }
    primary
        .multimodal
        .iter()
        .all(|tag| candidate.multimodal.contains(tag))
        && primary
            .task_capabilities
            .iter()
            .all(|tag| candidate.task_capabilities.contains(tag))
}

fn capability_rank(capability: &str) -> Option<u8> {
    match capability {
        "basic" => Some(0),
        "standard" => Some(1),
        "expert" => Some(2),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_models_from_llm_models_tables() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("corvid.toml");
        std::fs::write(
            &path,
            r#"
[llm.models.haiku]
provider = "anthropic"
capability = "basic"
version = "2024-10-22"
output_format = "strict_json"
privacy_tier = "hosted"
jurisdiction = "US"
latency_tier = "low"
context_window = 200000
structured_output = true
tool_calling = true
embeddings = false
multimodal = ["text", "image"]
task_capabilities = ["classification", "extraction"]
cost_per_token_in = "$0.00000025"
cost_per_token_out = 0.00000125

[llm.models.opus]
capability = "expert"
cost_per_token_in = 0.000015
"#,
        )
        .unwrap();

        let catalog = ModelCatalog::load_from_path(&path)
            .unwrap()
            .expect("catalog");
        assert_eq!(
            catalog.get("haiku").unwrap().provider.as_deref(),
            Some("anthropic")
        );
        assert_eq!(catalog.get("haiku").unwrap().capability.as_deref(), Some("basic"));
        assert_eq!(
            catalog.get("haiku").unwrap().version.as_deref(),
            Some("2024-10-22")
        );
        assert_eq!(
            catalog.get("haiku").unwrap().output_format.as_deref(),
            Some("strict_json")
        );
        assert_eq!(
            catalog.get("haiku").unwrap().privacy_tier.as_deref(),
            Some("hosted")
        );
        assert_eq!(
            catalog.get("haiku").unwrap().jurisdiction.as_deref(),
            Some("US")
        );
        assert_eq!(
            catalog.get("haiku").unwrap().latency_tier.as_deref(),
            Some("low")
        );
        assert_eq!(catalog.get("haiku").unwrap().context_window, Some(200000));
        assert!(catalog.get("haiku").unwrap().structured_output);
        assert!(catalog.get("haiku").unwrap().tool_calling);
        assert!(!catalog.get("haiku").unwrap().embeddings);
        assert_eq!(
            catalog.get("haiku").unwrap().multimodal,
            vec!["text".to_string(), "image".to_string()]
        );
        assert_eq!(
            catalog.get("haiku").unwrap().task_capabilities,
            vec!["classification".to_string(), "extraction".to_string()]
        );
        assert!((catalog.get("haiku").unwrap().cost_per_token_in - 0.00000025).abs() < 1e-12);
        assert!((catalog.get("opus").unwrap().cost_per_token_in - 0.000015).abs() < 1e-12);
    }

    #[test]
    fn chooses_cheapest_model_satisfying_capability() {
        let mut catalog = ModelCatalog::new();
        catalog.register(
            RegisteredModel::new("cheap-basic")
                .capability("basic")
                .cost_per_token_in(0.000001)
                .cost_per_token_out(0.000002),
        );
        catalog.register(
            RegisteredModel::new("cheap-expert")
                .provider("openai")
                .capability("expert")
                .privacy_tier("hosted")
                .jurisdiction("US")
                .context_window(128000)
                .structured_output(true)
                .task_capabilities(["classification", "ranking"])
                .cost_per_token_in(0.000002)
                .cost_per_token_out(0.000002),
        );
        catalog.register(
            RegisteredModel::new("expensive-expert")
                .capability("expert")
                .cost_per_token_in(0.00001)
                .cost_per_token_out(0.00001),
        );

        let selected = catalog
            .select_cheapest_by_capability("expert", 100, 50)
            .unwrap();
        assert_eq!(selected.model, "cheap-expert");
        assert_eq!(selected.capability_picked.as_deref(), Some("expert"));
        assert_eq!(selected.provider.as_deref(), Some("openai"));
        assert_eq!(selected.privacy_tier.as_deref(), Some("hosted"));
        assert_eq!(selected.jurisdiction.as_deref(), Some("US"));
        assert_eq!(selected.context_window, Some(128000));
        assert!(selected.structured_output);
        assert_eq!(
            selected.task_capabilities,
            vec!["classification".to_string(), "ranking".to_string()]
        );
    }

    #[test]
    fn chooses_cheapest_model_satisfying_output_format() {
        let mut catalog = ModelCatalog::new();
        catalog.register(
            RegisteredModel::new("markdown")
                .capability("expert")
                .output_format("markdown_strict")
                .cost_per_token_in(0.000001),
        );
        catalog.register(
            RegisteredModel::new("json-expensive")
                .capability("expert")
                .output_format("strict_json")
                .cost_per_token_in(0.000010),
        );
        catalog.register(
            RegisteredModel::new("json-cheap")
                .capability("standard")
                .output_format("strict_json")
                .cost_per_token_in(0.000002),
        );

        let selected = catalog
            .select_cheapest_by_requirements(None, Some("strict_json"), 100, 0)
            .unwrap();
        assert_eq!(selected.model, "json-cheap");
        assert_eq!(selected.output_format_picked.as_deref(), Some("strict_json"));
    }

    #[test]
    fn no_eligible_model_reports_requirement_and_available_models() {
        let mut catalog = ModelCatalog::new();
        catalog.register(RegisteredModel::new("cheap").capability("basic"));

        let err = catalog
            .select_cheapest_by_capability("expert", 10, 10)
            .unwrap_err();
        match err {
            RuntimeError::NoEligibleModel {
                required_capability,
                required_output_format,
                available_models,
            } => {
                assert_eq!(required_capability, "expert");
                assert_eq!(required_output_format, None);
                assert_eq!(available_models, vec!["cheap".to_string()]);
            }
            other => panic!("expected NoEligibleModel, got {other:?}"),
        }
    }

    #[test]
    fn describe_named_model_uses_catalog_metadata_when_present() {
        let mut catalog = ModelCatalog::new();
        catalog.register(
            RegisteredModel::new("opus")
                .capability("expert")
                .cost_per_token_in(0.1)
                .cost_per_token_out(0.2),
        );

        let selection = catalog.describe_named_model("opus", 2, 3);
        assert_eq!(selection.model, "opus");
        assert_eq!(selection.capability_picked.as_deref(), Some("expert"));
        assert_eq!(selection.version, None);
        assert_eq!(selection.output_format_picked, None);
        assert!((selection.cost_estimate - 0.8).abs() < 1e-12);
    }

    #[test]
    fn describe_named_model_falls_back_when_model_is_not_registered() {
        let selection = ModelCatalog::new().describe_named_model("custom", 10, 10);
        assert_eq!(selection.model, "custom");
        assert_eq!(selection.capability_picked, None);
        assert_eq!(selection.cost_estimate, 0.0);
    }

    #[test]
    fn compatible_fallbacks_preserve_contract_and_prefer_cheapest_other_provider() {
        let mut catalog = ModelCatalog::new();
        catalog.register(
            RegisteredModel::new("primary")
                .provider("openai")
                .capability("standard")
                .output_format("strict_json")
                .privacy_tier("hosted")
                .jurisdiction("US")
                .context_window(128000)
                .structured_output(true)
                .tool_calling(true)
                .multimodal(["text"])
                .task_capabilities(["classification"])
                .cost_per_token_in(0.000002),
        );
        catalog.register(
            RegisteredModel::new("fallback-expensive")
                .provider("anthropic")
                .capability("expert")
                .output_format("strict_json")
                .privacy_tier("hosted")
                .jurisdiction("US")
                .context_window(200000)
                .structured_output(true)
                .tool_calling(true)
                .multimodal(["text", "image"])
                .task_capabilities(["classification", "ranking"])
                .cost_per_token_in(0.000020),
        );
        catalog.register(
            RegisteredModel::new("fallback-cheap")
                .provider("gemini")
                .capability("standard")
                .output_format("strict_json")
                .privacy_tier("hosted")
                .jurisdiction("US")
                .context_window(128000)
                .structured_output(true)
                .tool_calling(true)
                .multimodal(["text"])
                .task_capabilities(["classification"])
                .cost_per_token_in(0.000001),
        );
        catalog.register(
            RegisteredModel::new("same-provider")
                .provider("openai")
                .capability("expert")
                .output_format("strict_json")
                .privacy_tier("hosted")
                .jurisdiction("US")
                .context_window(200000)
                .structured_output(true)
                .tool_calling(true)
                .multimodal(["text"])
                .task_capabilities(["classification"])
                .cost_per_token_in(0.0000001),
        );
        catalog.register(
            RegisteredModel::new("wrong-jurisdiction")
                .provider("ollama")
                .capability("expert")
                .output_format("strict_json")
                .privacy_tier("hosted")
                .jurisdiction("EU")
                .context_window(200000)
                .structured_output(true)
                .tool_calling(true)
                .multimodal(["text"])
                .task_capabilities(["classification"])
                .cost_per_token_in(0.0000001),
        );

        let fallbacks = catalog.compatible_fallbacks_for("primary", 100, 0);
        let names: Vec<_> = fallbacks
            .into_iter()
            .map(|selection| selection.model)
            .collect();
        assert_eq!(names, vec!["fallback-cheap", "fallback-expensive"]);
    }
}
