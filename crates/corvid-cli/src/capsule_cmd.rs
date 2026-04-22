use anyhow::{anyhow, bail, Context, Result};
use corvid_abi::{read_embedded_section_from_library, CorvidAbi, CORVID_ABI_VERSION};
use corvid_trace_schema::{read_events_from_path, schema_version_of, validate_supported_schema, TraceEvent};
use libloading::Library;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::ffi::{c_char, CString};
use std::fs::File;
use std::path::{Path, PathBuf};

const CAPSULE_FORMAT_VERSION: u32 = 1;
const MANIFEST_NAME: &str = "manifest.json";
const DESCRIPTOR_NAME: &str = "descriptor.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapsuleManifest {
    pub capsule_format_version: u32,
    pub runtime_version: String,
    pub compiler_version: String,
    pub trace_schema_version: u32,
    pub descriptor_abi_version: u32,
    pub deterministic_seed: u64,
    #[serde(default)]
    pub replay_model: Option<String>,
    pub library_file: String,
    pub descriptor_file: String,
    pub trace_file: String,
    pub library_sha256: String,
    pub descriptor_sha256: String,
    pub trace_sha256: String,
    pub replay_agent: String,
    pub replay_args: Vec<serde_json::Value>,
}

type CorvidCallAgentFn = unsafe extern "C" fn(
    *const c_char,
    *const c_char,
    usize,
    *mut *mut c_char,
    *mut usize,
    *mut u64,
    *mut CorvidApprovalRequired,
) -> u32;

type CorvidFreeResultFn = unsafe extern "C" fn(*mut c_char);
type CorvidObservationReleaseFn = unsafe extern "C" fn(u64);

#[repr(C)]
#[derive(Default)]
struct CorvidApprovalRequired {
    site_name: *const c_char,
    predicate_json: *const c_char,
    args_json: *const c_char,
    rationale_prompt: *const c_char,
}

pub fn run_create(trace: &Path, cdylib: &Path, out: Option<&Path>) -> Result<u8> {
    let events = read_events_from_path(trace)
        .with_context(|| format!("read trace `{}`", trace.display()))?;
    validate_supported_schema(&events)
        .with_context(|| format!("validate trace `{}`", trace.display()))?;
    let (agent, args) = last_run_started(&events)?;
    let deterministic_seed = derive_deterministic_seed(&events);
    let replay_model = last_recorded_model(&events);

    let section = read_embedded_section_from_library(cdylib)
        .with_context(|| format!("read embedded descriptor from `{}`", cdylib.display()))?;
    let descriptor: CorvidAbi = serde_json::from_str(&section.json)
        .with_context(|| format!("parse descriptor JSON from `{}`", cdylib.display()))?;

    let library_bytes = std::fs::read(cdylib)
        .with_context(|| format!("read library `{}`", cdylib.display()))?;
    let trace_bytes =
        std::fs::read(trace).with_context(|| format!("read trace `{}`", trace.display()))?;

    let library_name = cdylib
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("library path `{}` had no filename", cdylib.display()))?
        .to_string();
    let trace_name = trace
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("trace path `{}` had no filename", trace.display()))?
        .to_string();

    let manifest = CapsuleManifest {
        capsule_format_version: CAPSULE_FORMAT_VERSION,
        runtime_version: env!("CARGO_PKG_VERSION").to_string(),
        compiler_version: descriptor.compiler_version.clone(),
        trace_schema_version: schema_version_of(&events).unwrap_or(corvid_trace_schema::SCHEMA_VERSION),
        descriptor_abi_version: descriptor.corvid_abi_version,
        deterministic_seed,
        replay_model,
        library_file: library_name.clone(),
        descriptor_file: DESCRIPTOR_NAME.to_string(),
        trace_file: trace_name.clone(),
        library_sha256: encode_hex(&hash_bytes(&library_bytes)),
        descriptor_sha256: encode_hex(&hash_bytes(section.json.as_bytes())),
        trace_sha256: encode_hex(&hash_bytes(&trace_bytes)),
        replay_agent: agent,
        replay_args: args,
    };

    let output_path = out
        .map(PathBuf::from)
        .unwrap_or_else(|| trace.with_extension("capsule"));
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create capsule directory `{}`", parent.display()))?;
    }
    let file = File::create(&output_path)
        .with_context(|| format!("create capsule `{}`", output_path.display()))?;
    let mut builder = tar::Builder::new(file);
    append_bytes(&mut builder, &library_name, &library_bytes)?;
    append_bytes(&mut builder, DESCRIPTOR_NAME, section.json.as_bytes())?;
    append_bytes(&mut builder, &trace_name, &trace_bytes)?;
    let manifest_json = serde_json::to_vec_pretty(&manifest).context("serialize capsule manifest")?;
    append_bytes(&mut builder, MANIFEST_NAME, &manifest_json)?;
    builder.finish().context("finish capsule tarball")?;

    println!("{}", output_path.display());
    Ok(0)
}

pub fn run_replay(capsule: &Path) -> Result<u8> {
    let temp = tempfile::tempdir().context("create capsule tempdir")?;
    let file = File::open(capsule)
        .with_context(|| format!("open capsule `{}`", capsule.display()))?;
    let mut archive = tar::Archive::new(file);
    archive
        .unpack(temp.path())
        .with_context(|| format!("unpack capsule `{}`", capsule.display()))?;

    let manifest_path = temp.path().join(MANIFEST_NAME);
    let manifest: CapsuleManifest = serde_json::from_slice(
        &std::fs::read(&manifest_path)
            .with_context(|| format!("read capsule manifest `{}`", manifest_path.display()))?,
    )
    .with_context(|| format!("parse capsule manifest `{}`", manifest_path.display()))?;

    if manifest.capsule_format_version != CAPSULE_FORMAT_VERSION {
        bail!(
            "capsule format {} is unsupported by this CLI (supports {})",
            manifest.capsule_format_version,
            CAPSULE_FORMAT_VERSION
        );
    }

    let library_path = temp.path().join(&manifest.library_file);
    let descriptor_path = temp.path().join(&manifest.descriptor_file);
    let trace_path = temp.path().join(&manifest.trace_file);
    verify_capsule_file(&library_path, &manifest.library_sha256, "library")?;
    verify_capsule_file(&descriptor_path, &manifest.descriptor_sha256, "descriptor")?;
    verify_capsule_file(&trace_path, &manifest.trace_sha256, "trace")?;

    if manifest.runtime_version != env!("CARGO_PKG_VERSION") {
        eprintln!(
            "warning: capsule runtime version {} differs from current {}",
            manifest.runtime_version,
            env!("CARGO_PKG_VERSION")
        );
    }
    if manifest.descriptor_abi_version != CORVID_ABI_VERSION {
        bail!(
            "capsule descriptor ABI version {} is incompatible with current {}",
            manifest.descriptor_abi_version,
            CORVID_ABI_VERSION
        );
    }
    if manifest.trace_schema_version > corvid_trace_schema::SCHEMA_VERSION
        || manifest.trace_schema_version < corvid_trace_schema::MIN_SUPPORTED_SCHEMA
    {
        bail!(
            "capsule trace schema version {} is incompatible with current supported range {}..={}",
            manifest.trace_schema_version,
            corvid_trace_schema::MIN_SUPPORTED_SCHEMA,
            corvid_trace_schema::SCHEMA_VERSION
        );
    }

    let args_json = serde_json::Value::Array(manifest.replay_args.clone()).to_string();
    let output = unsafe {
        replay_library_call(
            &library_path,
            &trace_path,
            manifest.deterministic_seed,
            manifest.replay_model.as_deref(),
            &manifest.replay_agent,
            &args_json,
        )?
    };
    println!(
        "agent={} status={} result={} observation_handle={}",
        manifest.replay_agent, output.status, output.result_json, output.observation_handle
    );
    Ok(if output.status == 0 { 0 } else { 1 })
}

struct ReplayOutput {
    status: u32,
    result_json: String,
    observation_handle: u64,
}

unsafe fn replay_library_call(
    library_path: &Path,
    trace_path: &Path,
    deterministic_seed: u64,
    replay_model: Option<&str>,
    agent: &str,
    args_json: &str,
) -> Result<ReplayOutput> {
    let deterministic_seed_string = deterministic_seed.to_string();
    let mut entries: Vec<(&str, Option<&std::ffi::OsStr>)> = vec![
        ("CORVID_REPLAY_TRACE_PATH", Some(trace_path.as_os_str())),
        ("CORVID_TRACE_DISABLE", Some(std::ffi::OsStr::new("1"))),
        (
            "CORVID_DETERMINISTIC_SEED",
            Some(std::ffi::OsStr::new(&deterministic_seed_string)),
        ),
    ];
    if let Some(model) = replay_model {
        entries.push(("CORVID_MODEL", Some(std::ffi::OsStr::new(model))));
    }
    let _guard = EnvGuard::set(&entries);

    let library = Library::new(library_path)
        .with_context(|| format!("load library `{}`", library_path.display()))?;
    let call_agent: libloading::Symbol<CorvidCallAgentFn> = library
        .get(b"corvid_call_agent")
        .context("resolve corvid_call_agent")?;
    let free_result: libloading::Symbol<CorvidFreeResultFn> = library
        .get(b"corvid_free_result")
        .context("resolve corvid_free_result")?;
    let observation_release: Option<libloading::Symbol<CorvidObservationReleaseFn>> =
        library.get(b"corvid_observation_release").ok();

    let agent_c = CString::new(agent).context("agent name contained NUL")?;
    let args_c = CString::new(args_json).context("args JSON contained NUL")?;
    let mut result_ptr: *mut c_char = std::ptr::null_mut();
    let mut result_len: usize = 0;
    let mut observation_handle = 0u64;
    let mut approval = CorvidApprovalRequired::default();
    let status = call_agent(
        agent_c.as_ptr(),
        args_c.as_ptr(),
        args_json.len(),
        &mut result_ptr,
        &mut result_len,
        &mut observation_handle,
        &mut approval,
    );
    let result_json = if !result_ptr.is_null() {
        let text = std::slice::from_raw_parts(result_ptr as *const u8, result_len);
        let owned = String::from_utf8_lossy(text).into_owned();
        free_result(result_ptr);
        owned
    } else {
        "null".to_string()
    };
    if let Some(release) = observation_release {
        if observation_handle != 0 {
            release(observation_handle);
        }
    }
    Ok(ReplayOutput {
        status,
        result_json,
        observation_handle,
    })
}

fn append_bytes(builder: &mut tar::Builder<File>, path: &str, bytes: &[u8]) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder
        .append_data(&mut header, path, bytes)
        .with_context(|| format!("append `{path}` to capsule"))?;
    Ok(())
}

fn last_run_started(events: &[TraceEvent]) -> Result<(String, Vec<serde_json::Value>)> {
    events
        .iter()
        .rev()
        .find_map(|event| match event {
            TraceEvent::RunStarted { agent, args, .. } => Some((agent.clone(), args.clone())),
            _ => None,
        })
        .ok_or_else(|| anyhow!("trace had no run_started event"))
}

fn derive_deterministic_seed(events: &[TraceEvent]) -> u64 {
    events
        .iter()
        .rev()
        .find_map(|event| match event {
            TraceEvent::SeedRead { purpose, value, .. } if purpose == "rollout_default_seed" => {
                Some(*value)
            }
            _ => None,
        })
        .or_else(|| {
            events.iter().find_map(|event| match event {
                TraceEvent::SchemaHeader { ts_ms, .. } => Some(*ts_ms),
                _ => None,
            })
        })
        .unwrap_or(0)
}

fn last_recorded_model(events: &[TraceEvent]) -> Option<String> {
    events.iter().rev().find_map(|event| match event {
        TraceEvent::LlmCall {
            model: Some(model), ..
        }
        | TraceEvent::LlmResult {
            model: Some(model), ..
        } => Some(model.clone()),
        _ => None,
    })
}

fn hash_bytes(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

fn verify_capsule_file(path: &Path, expected_hex: &str, label: &str) -> Result<()> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("read capsule {label} file `{}`", path.display()))?;
    let actual = encode_hex(&hash_bytes(&bytes));
    if actual != expected_hex {
        bail!(
            "capsule {label} hash mismatch for `{}`: expected {}, got {}",
            path.display(),
            expected_hex,
            actual
        );
    }
    Ok(())
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

struct EnvGuard {
    saved: Vec<(String, Option<std::ffi::OsString>)>,
}

impl EnvGuard {
    fn set(entries: &[(&str, Option<&std::ffi::OsStr>)]) -> Self {
        let mut saved = Vec::with_capacity(entries.len());
        for (key, value) in entries {
            saved.push(((*key).to_string(), std::env::var_os(key)));
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
        Self { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.saved.drain(..).rev() {
            match value {
                Some(value) => std::env::set_var(&key, value),
                None => std::env::remove_var(&key),
            }
        }
    }
}
