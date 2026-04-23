use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{anyhow, bail, Context, Result};
use corvid_abi::{
    hash_json_str, AbiAgent, AbiDestructor, AbiOwnership, AbiOwnershipMode, AbiParam, CorvidAbi,
    ScalarTypeName, TypeDescription,
};

mod python_backend;
mod rust_backend;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindLanguage {
    Rust,
    Python,
}

impl FromStr for BindLanguage {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "rust" => Ok(Self::Rust),
            "python" | "py" => Ok(Self::Python),
            other => Err(format!(
                "unsupported binding language `{other}`; expected `rust` or `python`"
            )),
        }
    }
}

#[derive(Debug, Clone)]
pub struct GenerationOutput {
    pub files: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub(crate) struct GeneratedFile {
    pub relative_path: PathBuf,
    pub contents: String,
}

#[derive(Debug, Clone)]
pub(crate) struct BindingContext {
    pub abi: CorvidAbi,
    pub descriptor_hash_hex: String,
    pub package_name: String,
    pub package_dir_name: String,
    pub source_stem: String,
    pub bind_version: &'static str,
}

impl BindingContext {
    fn from_descriptor_json(
        descriptor_json: &str,
        descriptor_path: Option<&Path>,
    ) -> Result<Self> {
        let abi: CorvidAbi =
            serde_json::from_str(descriptor_json).context("parse Corvid ABI descriptor JSON")?;
        abi.validate_supported_version()
            .map_err(|err| anyhow!("unsupported Corvid ABI version: {err:?}"))?;
        validate_supported_ffi_surface(&abi)?;

        let hash = hash_json_str(descriptor_json);
        let descriptor_hash_hex = hex_bytes(&hash);
        let source_stem = source_stem(&abi, descriptor_path);
        let package_name = snake_case(&source_stem);
        let package_dir_name = package_name.replace('-', "_");

        Ok(Self {
            abi,
            descriptor_hash_hex,
            package_name,
            package_dir_name,
            source_stem,
            bind_version: env!("CARGO_PKG_VERSION"),
        })
    }
}

pub fn generate_bindings_from_descriptor_path(
    language: BindLanguage,
    descriptor_path: &Path,
    out_dir: &Path,
) -> Result<GenerationOutput> {
    let descriptor_json = fs::read_to_string(descriptor_path)
        .with_context(|| format!("read descriptor `{}`", descriptor_path.display()))?;
    let context = BindingContext::from_descriptor_json(&descriptor_json, Some(descriptor_path))?;
    generate_bindings(language, &context, out_dir)
}

fn generate_bindings(
    language: BindLanguage,
    context: &BindingContext,
    out_dir: &Path,
) -> Result<GenerationOutput> {
    let files = match language {
        BindLanguage::Rust => rust_backend::render(context)?,
        BindLanguage::Python => python_backend::render(context)?,
    };
    let mut written = Vec::with_capacity(files.len());
    for file in files {
        let destination = out_dir.join(&file.relative_path);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create output directory `{}`", parent.display()))?;
        }
        fs::write(&destination, file.contents)
            .with_context(|| format!("write `{}`", destination.display()))?;
        written.push(destination);
    }
    Ok(GenerationOutput { files: written })
}

pub(crate) fn validate_supported_ffi_surface(abi: &CorvidAbi) -> Result<()> {
    for agent in abi
        .agents
        .iter()
        .filter(|agent| is_generated_user_agent(agent))
    {
        for param in &agent.params {
            ensure_direct_scalar(agent, &param.ty, Some(&param.name))?;
        }
        ensure_supported_return(agent, &agent.return_type)?;
        if agent.attributes.dangerous && returns_grounded(&agent.return_type) {
            bail!(
                "agent `{}` returns Grounded<T> and requires approval; generated host bindings do not support dangerous grounded exports yet",
                agent.name
            );
        }
    }
    Ok(())
}

fn ensure_supported_return(agent: &AbiAgent, ty: &TypeDescription) -> Result<()> {
    match ty {
        TypeDescription::Grounded { grounded } => {
            ensure_direct_scalar(agent, grounded.inner.as_ref(), None)
                .with_context(|| format!("agent `{}` grounded return type", agent.name))
        }
        other => ensure_direct_scalar(agent, other, None)
            .with_context(|| format!("agent `{}` return type", agent.name)),
    }
}

fn ensure_direct_scalar(
    agent: &AbiAgent,
    ty: &TypeDescription,
    param_name: Option<&str>,
) -> Result<()> {
    if ffi_scalar_kind(ty).is_none() {
        if let Some(param_name) = param_name {
            bail!(
                "agent `{}` parameter `{}` has unsupported host-binding FFI type `{ty:?}`; only scalar pub extern \"c\" surfaces are supported in v1",
                agent.name,
                param_name
            );
        }
        bail!(
            "agent `{}` has unsupported host-binding FFI type `{ty:?}`; only scalar pub extern \"c\" surfaces are supported in v1",
            agent.name
        );
    }
    Ok(())
}

pub(crate) fn ffi_scalar_kind(ty: &TypeDescription) -> Option<ScalarTypeName> {
    match ty {
        TypeDescription::Scalar { scalar } => Some(scalar.clone()),
        _ => None,
    }
}

pub(crate) fn returns_grounded(ty: &TypeDescription) -> bool {
    matches!(ty, TypeDescription::Grounded { .. })
}

pub(crate) fn is_generated_user_agent(agent: &AbiAgent) -> bool {
    agent.attributes.pub_extern_c
        && !agent.name.starts_with("corvid_")
        && !agent.name.starts_with("__corvid_")
}

pub(crate) fn grounded_inner_kind(ty: &TypeDescription) -> Option<ScalarTypeName> {
    match ty {
        TypeDescription::Grounded { grounded } => ffi_scalar_kind(grounded.inner.as_ref()),
        _ => None,
    }
}

pub(crate) fn rust_public_type(ty: &TypeDescription) -> String {
    match ty {
        TypeDescription::Scalar {
            scalar: ScalarTypeName::Int,
        } => "i64".to_string(),
        TypeDescription::Scalar {
            scalar: ScalarTypeName::Float,
        } => "f64".to_string(),
        TypeDescription::Scalar {
            scalar: ScalarTypeName::String,
        } => "String".to_string(),
        TypeDescription::Scalar {
            scalar: ScalarTypeName::Bool,
        } => "bool".to_string(),
        TypeDescription::Scalar {
            scalar: ScalarTypeName::Nothing,
        } => "()".to_string(),
        TypeDescription::Scalar {
            scalar: ScalarTypeName::TraceId,
        } => "String".to_string(),
        TypeDescription::Struct { name } => pascal_case(name),
        TypeDescription::List { list } => format!("Vec<{}>", rust_public_type(list.element.as_ref())),
        TypeDescription::Result { result } => format!(
            "Result<{}, {}>",
            rust_public_type(result.ok.as_ref()),
            rust_public_type(result.err.as_ref())
        ),
        TypeDescription::Option { option } => {
            format!("Option<{}>", rust_public_type(option.inner.as_ref()))
        }
        TypeDescription::Grounded { grounded } => {
            format!("Grounded<{}>", rust_public_type(grounded.inner.as_ref()))
        }
        TypeDescription::Weak { weak } => {
            format!("Weak<{}>", rust_public_type(weak.inner.as_ref()))
        }
    }
}

pub(crate) fn effective_param_ownership(param: &AbiParam) -> AbiOwnership {
    param.ownership.clone().unwrap_or_else(|| match ffi_scalar_kind(&param.ty) {
        Some(ScalarTypeName::String | ScalarTypeName::TraceId) => AbiOwnership {
            mode: AbiOwnershipMode::Borrowed,
            lifetime: Some("call".to_string()),
            destructor: None,
        },
        _ => AbiOwnership {
            mode: AbiOwnershipMode::Owned,
            lifetime: None,
            destructor: None,
        },
    })
}

pub(crate) fn effective_return_ownership(agent: &AbiAgent) -> AbiOwnership {
    agent.return_ownership.clone().unwrap_or_else(|| match &agent.return_type {
        TypeDescription::Grounded { .. } => AbiOwnership {
            mode: AbiOwnershipMode::Owned,
            lifetime: None,
            destructor: Some(AbiDestructor {
                kind: corvid_abi::AbiDestructorKind::Release,
                symbol: "corvid_grounded_release".to_string(),
            }),
        },
        TypeDescription::Scalar {
            scalar: ScalarTypeName::String | ScalarTypeName::TraceId,
        } => AbiOwnership {
            mode: AbiOwnershipMode::Owned,
            lifetime: None,
            destructor: Some(AbiDestructor {
                kind: corvid_abi::AbiDestructorKind::Drop,
                symbol: "corvid_free_string".to_string(),
            }),
        },
        _ => AbiOwnership {
            mode: AbiOwnershipMode::Owned,
            lifetime: None,
            destructor: None,
        },
    })
}

pub(crate) fn rust_public_param_type(param: &AbiParam) -> String {
    match (
        ffi_scalar_kind(&param.ty),
        effective_param_ownership(param).mode,
        effective_param_ownership(param).lifetime,
    ) {
        (
            Some(ScalarTypeName::String | ScalarTypeName::TraceId),
            AbiOwnershipMode::Borrowed,
            Some(lifetime),
        ) if lifetime != "call" => format!("&'{lifetime} str"),
        (
            Some(ScalarTypeName::String | ScalarTypeName::TraceId),
            AbiOwnershipMode::Borrowed,
            _,
        ) => "&str".to_string(),
        _ => rust_public_type(&param.ty),
    }
}

pub(crate) fn python_public_type(ty: &TypeDescription) -> String {
    match ty {
        TypeDescription::Scalar {
            scalar: ScalarTypeName::Int,
        } => "int".to_string(),
        TypeDescription::Scalar {
            scalar: ScalarTypeName::Float,
        } => "float".to_string(),
        TypeDescription::Scalar {
            scalar: ScalarTypeName::String,
        } => "str".to_string(),
        TypeDescription::Scalar {
            scalar: ScalarTypeName::Bool,
        } => "bool".to_string(),
        TypeDescription::Scalar {
            scalar: ScalarTypeName::Nothing,
        } => "None".to_string(),
        TypeDescription::Scalar {
            scalar: ScalarTypeName::TraceId,
        } => "str".to_string(),
        TypeDescription::Struct { name } => pascal_case(name),
        TypeDescription::List { list } => {
            format!("list[{}]", python_public_type(list.element.as_ref()))
        }
        TypeDescription::Result { result } => format!(
            "tuple[{}, {}]",
            python_public_type(result.ok.as_ref()),
            python_public_type(result.err.as_ref())
        ),
        TypeDescription::Option { option } => {
            format!("{} | None", python_public_type(option.inner.as_ref()))
        }
        TypeDescription::Grounded { grounded } => {
            format!("Grounded[{}]", python_public_type(grounded.inner.as_ref()))
        }
        TypeDescription::Weak { weak } => {
            format!("Weak[{}]", python_public_type(weak.inner.as_ref()))
        }
    }
}

pub(crate) fn rust_c_abi_param_type(ty: &TypeDescription) -> Result<&'static str> {
    match ffi_scalar_kind(ty) {
        Some(ScalarTypeName::Int) => Ok("i64"),
        Some(ScalarTypeName::Float) => Ok("f64"),
        Some(ScalarTypeName::Bool) => Ok("bool"),
        Some(ScalarTypeName::String | ScalarTypeName::TraceId) => Ok("*const std::ffi::c_char"),
        Some(ScalarTypeName::Nothing) | None => bail!("unsupported direct C parameter type `{ty:?}`"),
    }
}

pub(crate) fn rust_c_abi_return_type(ty: &TypeDescription) -> Result<&'static str> {
    match ffi_scalar_kind(ty) {
        Some(ScalarTypeName::Int) => Ok("i64"),
        Some(ScalarTypeName::Float) => Ok("f64"),
        Some(ScalarTypeName::Bool) => Ok("bool"),
        Some(ScalarTypeName::String | ScalarTypeName::TraceId) => Ok("*const std::ffi::c_char"),
        Some(ScalarTypeName::Nothing) => Ok("()"),
        None => bail!("unsupported direct C return type `{ty:?}`"),
    }
}

pub(crate) fn python_ctypes_param_type(ty: &TypeDescription) -> Result<&'static str> {
    match ffi_scalar_kind(ty) {
        Some(ScalarTypeName::Int) => Ok("ctypes.c_int64"),
        Some(ScalarTypeName::Float) => Ok("ctypes.c_double"),
        Some(ScalarTypeName::Bool) => Ok("ctypes.c_bool"),
        Some(ScalarTypeName::String | ScalarTypeName::TraceId) => Ok("ctypes.c_char_p"),
        Some(ScalarTypeName::Nothing) | None => bail!("unsupported Python parameter type `{ty:?}`"),
    }
}

pub(crate) fn python_ctypes_return_type(ty: &TypeDescription) -> Result<&'static str> {
    match ffi_scalar_kind(ty) {
        Some(ScalarTypeName::Int) => Ok("ctypes.c_int64"),
        Some(ScalarTypeName::Float) => Ok("ctypes.c_double"),
        Some(ScalarTypeName::Bool) => Ok("ctypes.c_bool"),
        Some(ScalarTypeName::String | ScalarTypeName::TraceId) => Ok("ctypes.c_void_p"),
        Some(ScalarTypeName::Nothing) => Ok("None"),
        None => bail!("unsupported Python return type `{ty:?}`"),
    }
}

pub(crate) fn snake_case(value: &str) -> String {
    let mut out = String::new();
    let mut last_was_sep = false;
    let mut prev_was_lower_or_digit = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            if ch.is_ascii_uppercase() && prev_was_lower_or_digit && !last_was_sep {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
            last_was_sep = false;
            prev_was_lower_or_digit = ch.is_ascii_lowercase() || ch.is_ascii_digit();
        } else if !last_was_sep && !out.is_empty() {
            out.push('_');
            last_was_sep = true;
            prev_was_lower_or_digit = false;
        }
    }
    if out.is_empty() {
        "generated".to_string()
    } else {
        out.trim_matches('_').to_string()
    }
}

pub(crate) fn pascal_case(value: &str) -> String {
    let mut out = String::new();
    let mut uppercase_next = true;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            if uppercase_next {
                out.push(ch.to_ascii_uppercase());
                uppercase_next = false;
            } else {
                out.push(ch);
            }
        } else {
            uppercase_next = true;
        }
    }
    if out.is_empty() {
        "Generated".to_string()
    } else {
        out
    }
}

pub(crate) fn rust_string_literal(value: &str) -> String {
    format!("{value:?}")
}

pub(crate) fn rust_f64_literal(value: f64) -> String {
    let rendered = format!("{value:?}");
    if rendered.contains('.') || rendered.contains('e') || rendered.contains('E') {
        rendered
    } else {
        format!("{rendered}.0")
    }
}

pub(crate) fn python_string_literal(value: &str) -> String {
    let mut out = String::from("'");
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('\'');
    out
}

pub(crate) fn hex_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

fn source_stem(abi: &CorvidAbi, descriptor_path: Option<&Path>) -> String {
    if let Some(path) = descriptor_path.and_then(|path| path.file_stem()) {
        let stem = path.to_string_lossy();
        if let Some(stripped) = stem.strip_suffix(".corvid-abi") {
            return stripped.to_string();
        }
        return stem.to_string();
    }
    Path::new(&abi.source_path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| "corvid_generated".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snake_case_splits_words() {
        assert_eq!(snake_case("RefundBot"), "refund_bot");
        assert_eq!(snake_case("refund-bot"), "refund_bot");
    }

    #[test]
    fn pascal_case_normalizes_separators() {
        assert_eq!(pascal_case("refund_bot"), "RefundBot");
        assert_eq!(pascal_case("refund-bot"), "RefundBot");
    }
}
