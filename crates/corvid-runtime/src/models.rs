use crate::errors::RuntimeError;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub struct RegisteredModel {
    pub name: String,
    pub capability: Option<String>,
    pub version: Option<String>,
    pub output_format: Option<String>,
    pub cost_per_token_in: f64,
    pub cost_per_token_out: f64,
}

impl RegisteredModel {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            capability: None,
            version: None,
            output_format: None,
            cost_per_token_in: 0.0,
            cost_per_token_out: 0.0,
        }
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
    pub capability_required: Option<String>,
    pub capability_picked: Option<String>,
    pub version: Option<String>,
    pub output_format_required: Option<String>,
    pub output_format_picked: Option<String>,
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
            capability_required: required_capability.map(ToString::to_string),
            capability_picked: selected.capability.clone(),
            version: selected.version.clone(),
            output_format_required: required_output_format.map(ToString::to_string),
            output_format_picked: selected.output_format.clone(),
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
                capability_required: None,
                capability_picked: model.capability.clone(),
                version: model.version.clone(),
                output_format_required: None,
                output_format_picked: model.output_format.clone(),
                cost_estimate: model.estimated_cost(prompt_tokens, completion_tokens),
            },
            None => ModelSelection {
                model: model_name.to_string(),
                capability_required: None,
                capability_picked: None,
                version: None,
                output_format_required: None,
                output_format_picked: None,
                cost_estimate: 0.0,
            },
        }
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
        let version = spec
            .get("version")
            .and_then(toml::Value::as_str)
            .map(ToString::to_string);
        let output_format = spec
            .get("output_format")
            .and_then(toml::Value::as_str)
            .map(ToString::to_string);
        let cost_per_token_in = parse_cost_field(path, name, spec.get("cost_per_token_in"))?;
        let cost_per_token_out = parse_cost_field(path, name, spec.get("cost_per_token_out"))?;
        catalog.register(RegisteredModel {
            name: name.clone(),
            capability,
            version,
            output_format,
            cost_per_token_in,
            cost_per_token_out,
        });
    }
    Ok(catalog)
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
capability = "basic"
version = "2024-10-22"
output_format = "strict_json"
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
        assert_eq!(catalog.get("haiku").unwrap().capability.as_deref(), Some("basic"));
        assert_eq!(
            catalog.get("haiku").unwrap().version.as_deref(),
            Some("2024-10-22")
        );
        assert_eq!(
            catalog.get("haiku").unwrap().output_format.as_deref(),
            Some("strict_json")
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
                .capability("expert")
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
}
