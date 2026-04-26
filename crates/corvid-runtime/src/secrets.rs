use crate::errors::RuntimeError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretRead {
    pub name: String,
    pub present: bool,
    pub value: Option<String>,
}

#[derive(Clone, Default)]
pub struct SecretRuntime;

impl SecretRuntime {
    pub fn new() -> Self {
        Self
    }

    pub fn read_env(&self, name: impl Into<String>) -> Result<SecretRead, RuntimeError> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(RuntimeError::ToolFailed {
                tool: "std.secrets".to_string(),
                message: "secret name cannot be empty".to_string(),
            });
        }
        match std::env::var(&name) {
            Ok(value) => Ok(SecretRead {
                name,
                present: true,
                value: Some(value),
            }),
            Err(std::env::VarError::NotPresent) => Ok(SecretRead {
                name,
                present: false,
                value: None,
            }),
            Err(err) => Err(RuntimeError::ToolFailed {
                tool: "std.secrets".to_string(),
                message: format!("failed to read secret `{name}`: {err}"),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_runtime_reads_present_and_missing_env_without_leaking_missing_value() {
        std::env::set_var("CORVID_TEST_SECRET_RUNTIME", "secret-value");
        let runtime = SecretRuntime::new();
        let present = runtime.read_env("CORVID_TEST_SECRET_RUNTIME").unwrap();
        assert!(present.present);
        assert_eq!(present.value.as_deref(), Some("secret-value"));

        let missing = runtime.read_env("CORVID_TEST_SECRET_RUNTIME_MISSING").unwrap();
        assert!(!missing.present);
        assert_eq!(missing.value, None);
    }
}
