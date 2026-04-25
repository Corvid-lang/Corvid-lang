#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasmCodegenError {
    pub message: String,
}

impl std::fmt::Display for WasmCodegenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for WasmCodegenError {}

impl WasmCodegenError {
    pub(crate) fn unsupported(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}
