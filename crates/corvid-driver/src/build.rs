//! Build-to-disk helpers — compile a Corvid source file and write
//! the emitted artifact (Python or native binary) to `target/`.
//!
//! `corvid build <file>` and `corvid build --target native <file>`
//! both route through here.
//!
//! Extracted from `lib.rs` as part of Phase 20i responsibility
//! decomposition (20i-audit-driver-d).

use super::{
    compile_to_ir_with_config_at_path, compile_with_config_at_path, load_corvid_config_for,
    lower_driver_file, typecheck_driver_file, Diagnostic,
};
use corvid_ast::{
    AgentAttribute, Decl, Effect, EffectConstraint, EffectRow, ExtendMethodKind, File, PromptDecl,
    ToolDecl, TypeRef,
};
use corvid_guarantees::{lookup as lookup_guarantee, GuaranteeClass};
pub use corvid_codegen_cl::BuildTarget;
use corvid_ir::IrFile;
use corvid_resolve::{resolve, Resolved};
use corvid_syntax::{lex, parse_file};
use corvid_types::{Checked, CorvidConfig, EffectRegistry};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Compile `source_path` and write the generated Python to disk.
///
/// Layout convention:
///   * If the source is inside a `src/` directory, output goes to a sibling
///     `target/py/<stem>.py` relative to that `src/`.
///   * Otherwise, output goes alongside the source in `./target/py/<stem>.py`.
pub fn build_to_disk(source_path: &Path) -> anyhow::Result<BuildOutput> {
    let source = std::fs::read_to_string(source_path)
        .map_err(|e| anyhow::anyhow!("cannot read `{}`: {}", source_path.display(), e))?;

    let config = load_corvid_config_for(source_path);
    let result = compile_with_config_at_path(&source, source_path, config.as_ref());

    if !result.ok() {
        return Ok(BuildOutput {
            source,
            output_path: None,
            diagnostics: result.diagnostics,
        });
    }

    let out_path = output_path_for(source_path);
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let py = result.python_source.expect("codegen produced no source");
    std::fs::write(&out_path, &py)?;

    Ok(BuildOutput {
        source,
        output_path: Some(out_path),
        diagnostics: Vec::new(),
    })
}

pub struct BuildOutput {
    pub source: String,
    pub output_path: Option<PathBuf>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Compile `source_path` to a native binary under `<project>/target/bin/`.
///
/// Layout convention mirrors `build_to_disk`: if the source is inside a
/// `src/` directory, output goes to a sibling `target/bin/<stem>[.exe]`.
/// Otherwise, output goes alongside the source in `./target/bin/`.
pub fn build_native_to_disk(
    source_path: &Path,
    extra_tool_libs: &[&Path],
) -> anyhow::Result<NativeBuildOutput> {
    let source = std::fs::read_to_string(source_path)
        .map_err(|e| anyhow::anyhow!("cannot read `{}`: {}", source_path.display(), e))?;

    let config = load_corvid_config_for(source_path);
    match compile_to_ir_with_config_at_path(&source, source_path, config.as_ref()) {
        Err(diagnostics) => Ok(NativeBuildOutput {
            source,
            output_path: None,
            diagnostics,
        }),
        Ok(ir) => {
            let bin_dir = native_output_dir_for(source_path);
            let stem = source_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("program")
                .to_string();
            let requested = bin_dir.join(&stem);
            // Production users pass `--with-tools-lib` to the CLI;
            // this path is the one hit by that flow and by tool-free
            // `corvid build --target=native`.
            // Empty tools-lib list = no user tool crates linked — tool-using
            // programs fail at link time with an unresolved-symbol
            // error that surfaces the missing tool by name.
            let produced =
                corvid_codegen_cl::build_native_to_disk(&ir, &stem, &requested, extra_tool_libs)
                    .map_err(|e| anyhow::anyhow!("native codegen failed: {e}"))?;
            Ok(NativeBuildOutput {
                source,
                output_path: Some(produced),
                diagnostics: Vec::new(),
            })
        }
    }
}

pub struct NativeBuildOutput {
    pub source: String,
    pub output_path: Option<PathBuf>,
    pub diagnostics: Vec<Diagnostic>,
}

pub struct ServerBuildOutput {
    pub source: String,
    pub output_path: Option<PathBuf>,
    pub handler_path: Option<PathBuf>,
    pub diagnostics: Vec<Diagnostic>,
}

pub struct TargetBuildOutput {
    pub source: String,
    pub output_path: Option<PathBuf>,
    pub header_path: Option<PathBuf>,
    pub abi_descriptor_path: Option<PathBuf>,
    pub diagnostics: Vec<Diagnostic>,
    /// True when an ed25519 attestation envelope was signed and
    /// embedded into the cdylib at this build. False for unsigned
    /// builds, every non-cdylib target, and any frontend-error path.
    pub signed: bool,
}

/// Caller-provided signing material for the cdylib path. CLI parses
/// the key + label once at flag-parse time and hands the resolved
/// pair to the driver; the driver does not re-touch env vars or key
/// files itself.
pub struct SigningRequest {
    pub key: ed25519_dalek::SigningKey,
    pub key_id: String,
}

pub struct WasmBuildOutput {
    pub source: String,
    pub wasm_path: Option<PathBuf>,
    pub js_loader_path: Option<PathBuf>,
    pub ts_types_path: Option<PathBuf>,
    pub manifest_path: Option<PathBuf>,
    pub diagnostics: Vec<Diagnostic>,
}

pub struct AbiBuildOutput {
    pub source: String,
    pub descriptor_json: Option<String>,
    pub descriptor_hash: Option<[u8; 32]>,
    pub diagnostics: Vec<Diagnostic>,
}

struct FrontendBundle {
    source: String,
    file: corvid_ast::File,
    resolved: Resolved,
    checked: Checked,
    ir: IrFile,
    effect_registry: EffectRegistry,
}

struct CatalogDescriptorOutput {
    json: String,
    embedded_bytes: Vec<u8>,
}

fn validate_signed_claim_coverage(file: &File, descriptor_json: &str) -> anyhow::Result<()> {
    let descriptor = corvid_abi::descriptor_from_json(descriptor_json)
        .map_err(|err| anyhow::anyhow!("signed claim descriptor JSON is malformed: {err}"))?;
    let claim_ids = descriptor
        .claim_guarantees
        .iter()
        .map(|guarantee| guarantee.id.as_str())
        .collect::<BTreeSet<_>>();
    if claim_ids.is_empty() {
        return Err(anyhow::anyhow!(
            "`corvid build --sign` refused: ABI descriptor has no signed claim guarantee set"
        ));
    }

    let mut failures = Vec::new();
    for guarantee in &descriptor.claim_guarantees {
        match lookup_guarantee(&guarantee.id) {
            Some(registered) if registered.class != GuaranteeClass::OutOfScope => {}
            Some(_) => failures.push(format!(
                "descriptor claim `{}` is registered as out_of_scope",
                guarantee.id
            )),
            None => failures.push(format!(
                "descriptor claim `{}` is not registered in GUARANTEE_REGISTRY",
                guarantee.id
            )),
        }
    }

    let source_claims = declared_contract_claims(file);
    for id in &source_claims.required_ids {
        if !claim_ids.contains(id) {
            failures.push(format!(
                "source-declared contract requires `{id}`, but the signed descriptor claim set does not include it"
            ));
        }
    }
    failures.extend(source_claims.unsupported);

    if !failures.is_empty() {
        return Err(anyhow::anyhow!(
            "`corvid build --sign` refused because the signed ABI claim would be incomplete:\n  - {}",
            failures.join("\n  - ")
        ));
    }
    Ok(())
}

#[derive(Default)]
struct DeclaredContractClaims {
    required_ids: BTreeSet<&'static str>,
    unsupported: Vec<String>,
}

fn declared_contract_claims(file: &File) -> DeclaredContractClaims {
    let mut claims = DeclaredContractClaims::default();
    claims.required_ids.extend([
        "abi_descriptor.cdylib_emission",
        "abi_descriptor.byte_determinism",
        "abi_descriptor.bilateral_source_match",
        "abi_attestation.envelope_signature",
        "abi_attestation.descriptor_match",
        "abi_attestation.sign_requires_claim_coverage",
    ]);
    for decl in &file.decls {
        collect_decl_contracts(decl, &mut claims);
    }
    claims
}

fn collect_decl_contracts(decl: &Decl, claims: &mut DeclaredContractClaims) {
    match decl {
        Decl::Import(import) => {
            if !import.effect_row.is_empty()
                || !import.required_attributes.is_empty()
                || !import.required_constraints.is_empty()
            {
                claims.required_ids.insert("effect_row.import_boundary");
            }
        }
        Decl::Tool(tool) => collect_tool_contracts(tool, claims),
        Decl::Prompt(prompt) => collect_prompt_contracts(prompt, claims),
        Decl::Agent(agent) => {
            collect_effect_row_claims(&agent.effect_row, claims);
            collect_type_claims(&agent.return_ty, claims);
            for param in &agent.params {
                collect_type_claims(&param.ty, claims);
            }
            for constraint in &agent.constraints {
                collect_constraint_claims(&agent.name.name, constraint, claims);
            }
            for attr in &agent.attributes {
                match attr {
                    AgentAttribute::Replayable { .. } | AgentAttribute::Deterministic { .. } => {
                        claims.required_ids.insert("replay.deterministic_pure_path");
                    }
                    AgentAttribute::Wrapping { .. } => claims.unsupported.push(format!(
                        "agent `{}` declares `@wrapping`, but no signed cdylib guarantee id covers wrapping arithmetic yet",
                        agent.name.name
                    )),
                }
            }
        }
        Decl::Extend(extend) => {
            for method in &extend.methods {
                match &method.kind {
                    ExtendMethodKind::Tool(tool) => collect_tool_contracts(tool, claims),
                    ExtendMethodKind::Prompt(prompt) => collect_prompt_contracts(prompt, claims),
                    ExtendMethodKind::Agent(agent) => {
                        collect_decl_contracts(&Decl::Agent(agent.clone()), claims)
                    }
                }
            }
        }
        Decl::Eval(eval) => {
            for assertion in &eval.assertions {
                match assertion {
                    corvid_ast::EvalAssert::Value { confidence, .. } => {
                        if confidence.is_some() {
                            claims.required_ids.insert("confidence.min_threshold");
                        }
                    }
                    corvid_ast::EvalAssert::Approved { .. } => {
                        claims.required_ids.insert("approval.dangerous_call_requires_token");
                        claims.required_ids.insert("approval.token_lexical_only");
                    }
                    corvid_ast::EvalAssert::Cost { .. } => {
                        claims.required_ids.insert("budget.compile_time_ceiling");
                    }
                    corvid_ast::EvalAssert::Snapshot { .. }
                    | corvid_ast::EvalAssert::Called { .. }
                    | corvid_ast::EvalAssert::Ordering { .. } => {}
                }
            }
        }
        _ => {}
    }
}

fn collect_tool_contracts(tool: &ToolDecl, claims: &mut DeclaredContractClaims) {
    if tool.effect == Effect::Dangerous {
        claims.required_ids.insert("approval.dangerous_call_requires_token");
        claims.required_ids.insert("approval.token_lexical_only");
        claims.required_ids.insert("approval.dangerous_marker_preserved");
    }
    collect_effect_row_claims(&tool.effect_row, claims);
    collect_type_claims(&tool.return_ty, claims);
    for param in &tool.params {
        collect_type_claims(&param.ty, claims);
    }
}

fn collect_prompt_contracts(prompt: &PromptDecl, claims: &mut DeclaredContractClaims) {
    collect_effect_row_claims(&prompt.effect_row, claims);
    collect_type_claims(&prompt.return_ty, claims);
    for param in &prompt.params {
        collect_type_claims(&param.ty, claims);
    }
    if prompt.cites_strictly.is_some() {
        claims.required_ids.insert("grounded.provenance_required");
        claims.required_ids.insert("grounded.propagation_across_calls");
    }
    if prompt.stream.min_confidence.is_some() {
        claims.required_ids.insert("confidence.min_threshold");
    }
    if prompt.calibrated {
        claims.unsupported.push(format!(
            "prompt `{}` declares `calibrated`, but no signed cdylib guarantee id covers calibration yet",
            prompt.name.name
        ));
    }
    if prompt.cacheable {
        claims.unsupported.push(format!(
            "prompt `{}` declares `cacheable`, but no signed cdylib guarantee id covers prompt cache purity yet",
            prompt.name.name
        ));
    }
    if prompt.capability_required.is_some() {
        claims.unsupported.push(format!(
            "prompt `{}` declares `requires`, but no signed cdylib guarantee id covers model capability routing yet",
            prompt.name.name
        ));
    }
    if prompt.output_format_required.is_some() {
        claims.unsupported.push(format!(
            "prompt `{}` declares `output_format`, but no signed cdylib guarantee id covers output-format enforcement yet",
            prompt.name.name
        ));
    }
    if prompt.route.is_some()
        || prompt.progressive.is_some()
        || prompt.rollout.is_some()
        || prompt.ensemble.is_some()
        || prompt.adversarial.is_some()
    {
        claims.unsupported.push(format!(
            "prompt `{}` declares model dispatch policy, but no signed cdylib guarantee id covers dispatch correctness yet",
            prompt.name.name
        ));
    }
}

fn collect_effect_row_claims(row: &EffectRow, claims: &mut DeclaredContractClaims) {
    if !row.is_empty() {
        claims.required_ids.insert("effect_row.body_completeness");
        claims.required_ids.insert("effect_row.caller_propagation");
    }
}

fn collect_constraint_claims(
    agent_name: &str,
    constraint: &EffectConstraint,
    claims: &mut DeclaredContractClaims,
) {
    match constraint.dimension.name.as_str() {
        "budget" | "cost" => {
            claims.required_ids.insert("budget.compile_time_ceiling");
        }
        "min_confidence" | "confidence" => {
            claims.required_ids.insert("confidence.min_threshold");
        }
        other => claims.unsupported.push(format!(
            "agent `{agent_name}` declares `@{other}(...)`, but no signed cdylib guarantee id covers that effect constraint yet"
        )),
    }
}

fn collect_type_claims(ty: &TypeRef, claims: &mut DeclaredContractClaims) {
    if type_ref_contains_grounded(ty) {
        claims.required_ids.insert("grounded.provenance_required");
        claims.required_ids.insert("grounded.propagation_across_calls");
    }
}

fn type_ref_contains_grounded(ty: &TypeRef) -> bool {
    match ty {
        TypeRef::Named { name, .. } => name.name == "Grounded",
        TypeRef::Qualified { .. } => false,
        TypeRef::Generic { name, args, .. } => {
            name.name == "Grounded" || args.iter().any(type_ref_contains_grounded)
        }
        TypeRef::Weak { inner, .. } => type_ref_contains_grounded(inner),
        TypeRef::Function { params, ret, .. } => {
            params.iter().any(type_ref_contains_grounded) || type_ref_contains_grounded(ret)
        }
    }
}

pub fn build_target_to_disk(
    source_path: &Path,
    target: BuildTarget,
    emit_header: bool,
    emit_abi_descriptor: bool,
    extra_tool_libs: &[&Path],
    signing: Option<SigningRequest>,
) -> anyhow::Result<TargetBuildOutput> {
    if signing.is_some() && !matches!(target, BuildTarget::Cdylib) {
        return Err(anyhow::anyhow!(
            "signing is only supported for cdylib targets — descriptor attestations are bound to the embedded cdylib descriptor symbol"
        ));
    }
    let source = std::fs::read_to_string(source_path)
        .map_err(|e| anyhow::anyhow!("cannot read `{}`: {}", source_path.display(), e))?;

    let config = load_corvid_config_for(source_path);
    match build_frontend_bundle(&source, source_path, config.as_ref()) {
        Err(diagnostics) => Ok(TargetBuildOutput {
            source,
            output_path: None,
            header_path: None,
            abi_descriptor_path: None,
            diagnostics,
            signed: false,
        }),
        Ok(frontend) => {
            let out_dir = target_output_dir_for(source_path, target);
            let stem = source_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("program")
                .to_string();
            let requested = out_dir.join(&stem);
            let catalog_descriptor = if matches!(target, BuildTarget::Cdylib) {
                Some(emit_catalog_descriptor(source_path, &frontend)?)
            } else {
                None
            };
            if signing.is_some() {
                let descriptor = catalog_descriptor
                    .as_ref()
                    .expect("signed builds are only supported for cdylib descriptors");
                validate_signed_claim_coverage(&frontend.file, &descriptor.json)?;
            }
            // Sign the descriptor JSON now so the envelope is locked
            // before any codegen happens. The DSSE PAE binds the
            // signature to (payloadType, payload), so even if the
            // verifier later sees a binary with a tampered descriptor
            // section, the signature won't match the recovered
            // payload.
            let attestation_bytes = match (&catalog_descriptor, &signing) {
                (Some(descriptor), Some(req)) => {
                    let envelope = corvid_abi::sign_envelope(
                        descriptor.json.as_bytes(),
                        corvid_abi::CORVID_ABI_ATTESTATION_PAYLOAD_TYPE,
                        &req.key,
                        &req.key_id,
                    );
                    let envelope_json = serde_json::to_vec(&envelope)
                        .map_err(|e| anyhow::anyhow!("serialize attestation envelope: {e}"))?;
                    Some(corvid_abi::attestation_to_embedded_bytes(&envelope_json))
                }
                _ => None,
            };
            let signed = attestation_bytes.is_some();
            let produced = match target {
                BuildTarget::Native => corvid_codegen_cl::build_native_to_disk(
                    &frontend.ir,
                    &stem,
                    &requested,
                    extra_tool_libs,
                ),
                BuildTarget::Cdylib | BuildTarget::Staticlib => {
                    corvid_codegen_cl::build_library_to_disk(
                        &frontend.ir,
                        &stem,
                        &requested,
                        target,
                        extra_tool_libs,
                        catalog_descriptor
                            .as_ref()
                            .map(|descriptor| descriptor.embedded_bytes.as_slice()),
                        attestation_bytes.as_deref(),
                    )
                }
            }
            .map_err(|e| anyhow::anyhow!("native codegen failed: {e}"))?;

            let header_path = if emit_header {
                let header = corvid_c_header::emit_header(
                    &frontend.ir,
                    &corvid_c_header::HeaderOptions {
                        library_name: stem.clone(),
                    },
                );
                let path = out_dir.join(format!("lib_{stem}.h"));
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, header)?;
                Some(path)
            } else {
                None
            };

            let abi_descriptor_path = if emit_abi_descriptor {
                let descriptor_json = if let Some(descriptor) = &catalog_descriptor {
                    descriptor.json.clone()
                } else {
                    emit_catalog_descriptor(source_path, &frontend)?.json
                };
                let path = out_dir.join(format!("{stem}.corvid-abi.json"));
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, descriptor_json)?;
                Some(path)
            } else {
                None
            };

            Ok(TargetBuildOutput {
                source,
                output_path: Some(produced),
                header_path,
                abi_descriptor_path,
                diagnostics: Vec::new(),
                signed,
            })
        }
    }
}

pub fn build_wasm_to_disk(source_path: &Path) -> anyhow::Result<WasmBuildOutput> {
    let source = std::fs::read_to_string(source_path)
        .map_err(|e| anyhow::anyhow!("cannot read `{}`: {}", source_path.display(), e))?;

    let config = load_corvid_config_for(source_path);
    match build_frontend_bundle(&source, source_path, config.as_ref()) {
        Err(diagnostics) => Ok(WasmBuildOutput {
            source,
            wasm_path: None,
            js_loader_path: None,
            ts_types_path: None,
            manifest_path: None,
            diagnostics,
        }),
        Ok(frontend) => {
            let out_dir = wasm_output_dir_for(source_path);
            let stem = source_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("program")
                .to_string();
            let artifacts = corvid_codegen_wasm::emit_wasm_artifacts(&frontend.ir, &stem)
                .map_err(|e| anyhow::anyhow!("wasm codegen failed: {e}"))?;
            std::fs::create_dir_all(&out_dir)?;
            let wasm_path = out_dir.join(format!("{stem}.wasm"));
            let js_loader_path = out_dir.join(format!("{stem}.js"));
            let ts_types_path = out_dir.join(format!("{stem}.d.ts"));
            let manifest_path = out_dir.join(format!("{stem}.corvid-wasm.json"));
            std::fs::write(&wasm_path, artifacts.wasm)?;
            std::fs::write(&js_loader_path, artifacts.js_loader)?;
            std::fs::write(&ts_types_path, artifacts.ts_types)?;
            std::fs::write(&manifest_path, artifacts.manifest_json)?;
            Ok(WasmBuildOutput {
                source,
                wasm_path: Some(wasm_path),
                js_loader_path: Some(js_loader_path),
                ts_types_path: Some(ts_types_path),
                manifest_path: Some(manifest_path),
                diagnostics: Vec::new(),
            })
        }
    }
}

pub fn build_server_to_disk(
    source_path: &Path,
    extra_tool_libs: &[&Path],
) -> anyhow::Result<ServerBuildOutput> {
    let native = build_native_to_disk(source_path, extra_tool_libs)?;
    let Some(handler_path) = native.output_path else {
        return Ok(ServerBuildOutput {
            source: native.source,
            output_path: None,
            handler_path: None,
            diagnostics: native.diagnostics,
        });
    };

    let server_dir = server_output_dir_for(source_path);
    std::fs::create_dir_all(&server_dir)?;
    let stem = source_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("program");
    let source_rs = server_dir.join(format!("{stem}_server.rs"));
    let output_path = server_binary_path_for(&server_dir, stem);
    std::fs::write(&source_rs, render_minimal_server_source(&handler_path))?;

    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let status = std::process::Command::new(rustc)
        .arg("--edition=2021")
        .arg(&source_rs)
        .arg("-o")
        .arg(&output_path)
        .status()
        .map_err(|err| anyhow::anyhow!("failed to invoke rustc for server wrapper: {err}"))?;
    if !status.success() {
        return Err(anyhow::anyhow!(
            "server wrapper compilation failed with status {status}"
        ));
    }

    Ok(ServerBuildOutput {
        source: native.source,
        output_path: Some(output_path),
        handler_path: Some(handler_path),
        diagnostics: Vec::new(),
    })
}

pub fn build_catalog_descriptor_for_source(source_path: &Path) -> anyhow::Result<AbiBuildOutput> {
    let source = std::fs::read_to_string(source_path)
        .map_err(|e| anyhow::anyhow!("cannot read `{}`: {}", source_path.display(), e))?;
    let config = load_corvid_config_for(source_path);
    match build_frontend_bundle(&source, source_path, config.as_ref()) {
        Err(diagnostics) => Ok(AbiBuildOutput {
            source,
            descriptor_json: None,
            descriptor_hash: None,
            diagnostics,
        }),
        Ok(frontend) => {
            let descriptor = emit_catalog_descriptor(source_path, &frontend)?;
            let hash = corvid_abi::hash_json_str(&descriptor.json);
            Ok(AbiBuildOutput {
                source,
                descriptor_json: Some(descriptor.json),
                descriptor_hash: Some(hash),
                diagnostics: Vec::new(),
            })
        }
    }
}

fn emit_catalog_descriptor(
    source_path: &Path,
    frontend: &FrontendBundle,
) -> anyhow::Result<CatalogDescriptorOutput> {
    // Phase 22-C embeds and hashes the descriptor inside the produced cdylib,
    // so the JSON body must be byte-stable across identical builds.
    let generated_at = "1970-01-01T00:00:00Z".to_string();
    let normalized_source_path = corvid_abi::normalize_source_path(&source_path.to_string_lossy());
    let descriptor = corvid_abi::emit_catalog_abi(
        &frontend.file,
        &frontend.resolved,
        &frontend.checked,
        &frontend.ir,
        &frontend.effect_registry,
        &corvid_abi::EmitOptions {
            source_path: &normalized_source_path,
            source_text: &frontend.source,
            compiler_version: env!("CARGO_PKG_VERSION"),
            generated_at: &generated_at,
        },
    );
    let json = corvid_abi::render_descriptor_json(&descriptor)
        .map_err(|e| anyhow::anyhow!("serialize descriptor: {e}"))?;
    let embedded_bytes = corvid_abi::descriptor_to_embedded_bytes(&descriptor)
        .map_err(|e| anyhow::anyhow!("encode embedded descriptor: {e}"))?;
    Ok(CatalogDescriptorOutput {
        json,
        embedded_bytes,
    })
}

fn build_frontend_bundle(
    source: &str,
    source_path: &Path,
    config: Option<&CorvidConfig>,
) -> Result<FrontendBundle, Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();
    let tokens = match lex(source) {
        Ok(tokens) => tokens,
        Err(errs) => {
            diagnostics.extend(errs.into_iter().map(Diagnostic::from));
            return Err(diagnostics);
        }
    };
    let (file, parse_errs) = parse_file(&tokens);
    diagnostics.extend(parse_errs.into_iter().map(Diagnostic::from));
    let resolved = resolve(&file);
    diagnostics.extend(resolved.errors.iter().cloned().map(Diagnostic::from));
    let typechecked = typecheck_driver_file(&file, &resolved, source_path, config);
    diagnostics.extend(typechecked.diagnostics);
    diagnostics.extend(
        typechecked
            .result
            .checked
            .errors
            .iter()
            .cloned()
            .map(Diagnostic::from),
    );
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    let effect_decls = file
        .decls
        .iter()
        .filter_map(|decl| match decl {
            corvid_ast::Decl::Effect(effect) => Some(effect.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let effect_registry = EffectRegistry::from_decls_with_config(&effect_decls, config);
    let checked = typechecked.result.checked.clone();
    let ir = lower_driver_file(&file, &resolved, &typechecked.result);
    Ok(FrontendBundle {
        source: source.to_string(),
        file,
        resolved,
        checked,
        ir,
        effect_registry,
    })
}

pub(super) fn native_output_dir_for(source_path: &Path) -> PathBuf {
    let mut ancestor: Option<&Path> = source_path.parent();
    while let Some(dir) = ancestor {
        if dir.file_name().map(|n| n == "src").unwrap_or(false) {
            if let Some(project_root) = dir.parent() {
                return project_root.join("target").join("bin");
            }
        }
        ancestor = dir.parent();
    }
    let parent = source_path.parent().unwrap_or_else(|| Path::new("."));
    parent.join("target").join("bin")
}

pub(super) fn target_output_dir_for(source_path: &Path, target: BuildTarget) -> PathBuf {
    match target {
        BuildTarget::Native => native_output_dir_for(source_path),
        BuildTarget::Cdylib | BuildTarget::Staticlib => {
            let mut ancestor: Option<&Path> = source_path.parent();
            while let Some(dir) = ancestor {
                if dir.file_name().map(|n| n == "src").unwrap_or(false) {
                    if let Some(project_root) = dir.parent() {
                        return project_root.join("target").join("release");
                    }
                }
                ancestor = dir.parent();
            }
            let parent = source_path.parent().unwrap_or_else(|| Path::new("."));
            parent.join("target").join("release")
        }
    }
}

pub(super) fn server_output_dir_for(source_path: &Path) -> PathBuf {
    let mut ancestor: Option<&Path> = source_path.parent();
    while let Some(dir) = ancestor {
        if dir.file_name().map(|n| n == "src").unwrap_or(false) {
            if let Some(project_root) = dir.parent() {
                return project_root.join("target").join("server");
            }
        }
        ancestor = dir.parent();
    }
    let parent = source_path.parent().unwrap_or_else(|| Path::new("."));
    parent.join("target").join("server")
}

fn server_binary_path_for(out_dir: &Path, stem: &str) -> PathBuf {
    if cfg!(windows) {
        out_dir.join(format!("{stem}_server.exe"))
    } else {
        out_dir.join(format!("{stem}_server"))
    }
}

fn render_minimal_server_source(handler_path: &Path) -> String {
    let handler = handler_path.to_string_lossy().replace('\\', "\\\\");
    format!(
        r#"use std::io::{{Read, Write}};
use std::net::{{TcpListener, TcpStream}};
use std::process::Command;
use std::time::Instant;

const HANDLER: &str = "{handler}";

fn main() -> std::io::Result<()> {{
    let host = std::env::var("CORVID_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = std::env::var("CORVID_PORT").unwrap_or_else(|_| "8080".to_string());
    let listener = TcpListener::bind(format!("{{host}}:{{port}}"))?;
    let addr = listener.local_addr()?;
    println!("listening: http://{{addr}}");
    for stream in listener.incoming() {{
        match stream {{
            Ok(stream) => {{
                let _ = handle(stream);
            }}
            Err(err) => eprintln!("accept error: {{err}}"),
        }}
    }}
    Ok(())
}}

fn handle(mut stream: TcpStream) -> std::io::Result<()> {{
    let started = Instant::now();
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf)?;
    let req = String::from_utf8_lossy(&buf[..n]);
    let first = req.lines().next().unwrap_or("");
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("/");
    if method != "GET" {{
        return respond(&mut stream, 405, "text/plain", "method not allowed", started);
    }}
    if path == "/healthz" {{
        return respond(&mut stream, 200, "application/json", "{{\"status\":\"ok\"}}", started);
    }}
    let output = Command::new(HANDLER).output();
    match output {{
        Ok(out) if out.status.success() => {{
            let body = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let json = format!("{{{{\"result\":{{:?}}}}}}", body);
            respond(&mut stream, 200, "application/json", &json, started)
        }}
        Ok(out) => {{
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            let body = format!("{{{{\"error\":{{:?}}}}}}", err);
            respond(&mut stream, 500, "application/json", &body, started)
        }}
        Err(err) => {{
            let body = format!("{{{{\"error\":{{:?}}}}}}", err.to_string());
            respond(&mut stream, 500, "application/json", &body, started)
        }}
    }}
}}

fn respond(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &str,
    started: Instant,
) -> std::io::Result<()> {{
    let reason = match status {{
        200 => "OK",
        405 => "Method Not Allowed",
        _ => "Internal Server Error",
    }};
    let request_id = format!("req-{{:?}}", started.elapsed().as_nanos());
    let response = format!(
        "HTTP/1.1 {{status}} {{reason}}\r\ncontent-type: {{content_type}}\r\ncontent-length: {{}}\r\nx-corvid-request-id: {{request_id}}\r\nconnection: close\r\n\r\n{{body}}",
        body.as_bytes().len()
    );
    stream.write_all(response.as_bytes())
}}
"#
    )
}

pub(super) fn wasm_output_dir_for(source_path: &Path) -> PathBuf {
    let mut ancestor: Option<&Path> = source_path.parent();
    while let Some(dir) = ancestor {
        if dir.file_name().map(|n| n == "src").unwrap_or(false) {
            if let Some(project_root) = dir.parent() {
                return project_root.join("target").join("wasm");
            }
        }
        ancestor = dir.parent();
    }
    let parent = source_path.parent().unwrap_or_else(|| Path::new("."));
    parent.join("target").join("wasm")
}

pub(super) fn output_path_for(source_path: &Path) -> PathBuf {
    let stem = source_path.file_stem().unwrap_or_default();
    let py_name = format!("{}.py", stem.to_string_lossy());

    // Find the nearest enclosing `src` directory by walking up.
    let mut ancestor: Option<&Path> = source_path.parent();
    while let Some(dir) = ancestor {
        if dir.file_name().map(|n| n == "src").unwrap_or(false) {
            if let Some(project_root) = dir.parent() {
                return project_root.join("target").join("py").join(py_name);
            }
        }
        ancestor = dir.parent();
    }

    // Default: alongside the source, in a `target/py/` subdir.
    let parent = source_path.parent().unwrap_or_else(|| Path::new("."));
    parent.join("target").join("py").join(py_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_source(source: &str) -> File {
        let tokens = lex(source).expect("lex");
        let (file, errors) = parse_file(&tokens);
        assert!(errors.is_empty(), "parse errors: {errors:?}");
        file
    }

    fn descriptor_with_claim_ids(ids: &[&str]) -> String {
        let claims = ids
            .iter()
            .map(|id| {
                let guarantee = lookup_guarantee(id).expect("registered guarantee");
                serde_json::json!({
                    "id": guarantee.id,
                    "kind": guarantee.kind.slug(),
                    "class": guarantee.class.slug(),
                    "phase": guarantee.phase.slug(),
                })
            })
            .collect::<Vec<_>>();
        serde_json::json!({
            "corvid_abi_version": corvid_abi::CORVID_ABI_VERSION,
            "compiler_version": "test",
            "source_path": "test.cor",
            "generated_at": "1970-01-01T00:00:00Z",
            "agents": [],
            "prompts": [],
            "tools": [],
            "types": [],
            "stores": [],
            "approval_sites": [],
            "claim_guarantees": claims,
        })
        .to_string()
    }

    #[test]
    fn signed_claim_coverage_accepts_registered_contracts() {
        let file = parse_source(
            r#"
effect transfer:
    cost: $0.01

tool issue_refund(id: String) -> String dangerous uses transfer

@budget($0.50)
@replayable
pub extern "c"
agent refund(id: String) -> String uses transfer:
    approve issue_refund(id)
    return issue_refund(id)
"#,
        );
        let descriptor =
            descriptor_with_claim_ids(corvid_guarantees::SIGNED_CDYLIB_CLAIM_GUARANTEE_IDS);
        validate_signed_claim_coverage(&file, &descriptor).expect("coverage accepted");
    }

    #[test]
    fn signed_claim_coverage_rejects_missing_declared_contract_id() {
        let file = parse_source(
            r#"
tool issue_refund(id: String) -> String dangerous

pub extern "c"
agent refund(id: String) -> String:
    approve issue_refund(id)
    return issue_refund(id)
"#,
        );
        let ids = corvid_guarantees::SIGNED_CDYLIB_CLAIM_GUARANTEE_IDS
            .iter()
            .copied()
            .filter(|id| *id != "approval.dangerous_call_requires_token")
            .collect::<Vec<_>>();
        let descriptor = descriptor_with_claim_ids(&ids);
        let err = validate_signed_claim_coverage(&file, &descriptor)
            .expect_err("missing approval claim must reject signing");
        assert!(
            err.to_string()
                .contains("approval.dangerous_call_requires_token"),
            "{err:#}"
        );
    }

    #[test]
    fn signed_claim_coverage_rejects_out_of_scope_contract_id() {
        let file = parse_source(
            r#"
pub extern "c"
agent answer(x: Int) -> Int:
    return x
"#,
        );
        let mut ids = corvid_guarantees::SIGNED_CDYLIB_CLAIM_GUARANTEE_IDS.to_vec();
        ids.push("platform.signing_key_compromise");
        let descriptor = descriptor_with_claim_ids(&ids);
        let err = validate_signed_claim_coverage(&file, &descriptor)
            .expect_err("out-of-scope claim must reject signing");
        assert!(
            err.to_string().contains("out_of_scope"),
            "{err:#}"
        );
    }
}
