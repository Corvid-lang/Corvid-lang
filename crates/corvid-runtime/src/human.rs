//! Human input primitives for `ask` and `choose`.
//!
//! These are separate from approvals: approval decides whether an effect may
//! proceed, while human input supplies typed data or selects among values.

use crate::errors::RuntimeError;
use futures::future::BoxFuture;
use serde_json::Value;
use std::collections::VecDeque;
use std::io::Write;
use std::sync::{Arc, Mutex};

pub trait HumanInteractor: Send + Sync {
    fn ask<'a>(&'a self, req: &'a HumanInputRequest) -> BoxFuture<'a, Result<Value, RuntimeError>>;

    fn choose<'a>(
        &'a self,
        req: &'a HumanChoiceRequest,
    ) -> BoxFuture<'a, Result<usize, RuntimeError>>;
}

#[derive(Debug, Clone)]
pub struct HumanInputRequest {
    pub prompt: String,
    pub expected_type: String,
}

#[derive(Debug, Clone)]
pub struct HumanChoiceRequest {
    pub options: Vec<Value>,
}

pub struct StdinHumanInteractor;

impl StdinHumanInteractor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for StdinHumanInteractor {
    fn default() -> Self {
        Self::new()
    }
}

impl HumanInteractor for StdinHumanInteractor {
    fn ask<'a>(&'a self, req: &'a HumanInputRequest) -> BoxFuture<'a, Result<Value, RuntimeError>> {
        Box::pin(async move {
            let req = req.clone();
            tokio::task::spawn_blocking(move || {
                let stdout = std::io::stdout();
                let mut handle = stdout.lock();
                writeln!(handle, "corvid ask [{}]", req.expected_type)
                    .map_err(|err| RuntimeError::Other(err.to_string()))?;
                write!(handle, "{}: ", req.prompt)
                    .map_err(|err| RuntimeError::Other(err.to_string()))?;
                handle.flush().ok();

                let mut line = String::new();
                std::io::stdin()
                    .read_line(&mut line)
                    .map_err(|err| RuntimeError::Other(err.to_string()))?;
                parse_human_value(line.trim(), &req.expected_type)
            })
            .await
            .map_err(|err| RuntimeError::Other(format!("human input task failed: {err}")))?
        })
    }

    fn choose<'a>(
        &'a self,
        req: &'a HumanChoiceRequest,
    ) -> BoxFuture<'a, Result<usize, RuntimeError>> {
        Box::pin(async move {
            let req = req.clone();
            tokio::task::spawn_blocking(move || {
                if req.options.is_empty() {
                    return Err(RuntimeError::Other(
                        "choose requires at least one option".into(),
                    ));
                }
                let stdout = std::io::stdout();
                let mut handle = stdout.lock();
                writeln!(handle, "corvid choose")
                    .map_err(|err| RuntimeError::Other(err.to_string()))?;
                for (index, option) in req.options.iter().enumerate() {
                    writeln!(handle, "  {index}: {option}")
                        .map_err(|err| RuntimeError::Other(err.to_string()))?;
                }
                write!(handle, "select option index: ")
                    .map_err(|err| RuntimeError::Other(err.to_string()))?;
                handle.flush().ok();

                let mut line = String::new();
                std::io::stdin()
                    .read_line(&mut line)
                    .map_err(|err| RuntimeError::Other(err.to_string()))?;
                let index = line
                    .trim()
                    .parse::<usize>()
                    .map_err(|err| RuntimeError::Other(format!("invalid choice index: {err}")))?;
                if index >= req.options.len() {
                    return Err(RuntimeError::Other(format!(
                        "choice index {index} out of range for {} options",
                        req.options.len()
                    )));
                }
                Ok(index)
            })
            .await
            .map_err(|err| RuntimeError::Other(format!("human choice task failed: {err}")))?
        })
    }
}

pub struct ProgrammaticHumanInteractor {
    answers: Mutex<VecDeque<Value>>,
    choices: Mutex<VecDeque<usize>>,
}

impl ProgrammaticHumanInteractor {
    pub fn new(
        answers: impl IntoIterator<Item = Value>,
        choices: impl IntoIterator<Item = usize>,
    ) -> Self {
        Self {
            answers: Mutex::new(answers.into_iter().collect()),
            choices: Mutex::new(choices.into_iter().collect()),
        }
    }
}

impl HumanInteractor for ProgrammaticHumanInteractor {
    fn ask<'a>(
        &'a self,
        _req: &'a HumanInputRequest,
    ) -> BoxFuture<'a, Result<Value, RuntimeError>> {
        Box::pin(async move {
            self.answers
                .lock()
                .expect("human answers lock poisoned")
                .pop_front()
                .ok_or_else(|| RuntimeError::Other("no programmatic human answer available".into()))
        })
    }

    fn choose<'a>(
        &'a self,
        req: &'a HumanChoiceRequest,
    ) -> BoxFuture<'a, Result<usize, RuntimeError>> {
        Box::pin(async move {
            let index = self
                .choices
                .lock()
                .expect("human choices lock poisoned")
                .pop_front()
                .ok_or_else(|| {
                    RuntimeError::Other("no programmatic human choice available".into())
                })?;
            if index >= req.options.len() {
                return Err(RuntimeError::Other(format!(
                    "choice index {index} out of range for {} options",
                    req.options.len()
                )));
            }
            Ok(index)
        })
    }
}

impl From<ProgrammaticHumanInteractor> for Arc<dyn HumanInteractor> {
    fn from(value: ProgrammaticHumanInteractor) -> Self {
        Arc::new(value)
    }
}

fn parse_human_value(raw: &str, expected_type: &str) -> Result<Value, RuntimeError> {
    match expected_type {
        "Int" => raw
            .parse::<i64>()
            .map(Value::from)
            .map_err(|err| RuntimeError::Other(format!("invalid Int input: {err}"))),
        "Float" => raw
            .parse::<f64>()
            .map(Value::from)
            .map_err(|err| RuntimeError::Other(format!("invalid Float input: {err}"))),
        "Bool" => match raw.to_ascii_lowercase().as_str() {
            "true" | "yes" | "y" => Ok(Value::Bool(true)),
            "false" | "no" | "n" => Ok(Value::Bool(false)),
            _ => Err(RuntimeError::Other("invalid Bool input".into())),
        },
        "Nothing" => Ok(Value::Null),
        "String" => Ok(Value::String(raw.to_string())),
        _ => serde_json::from_str(raw).map_err(|err| {
            RuntimeError::Other(format!(
                "expected JSON for `{expected_type}` human input: {err}"
            ))
        }),
    }
}
