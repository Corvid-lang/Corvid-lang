//! Pipeline orchestration: parse → resolve → typecheck → lower → codegen.
//!
//! Driver is the CLI's library. The `corvid` binary thinly wraps these
//! functions. Kept small so it's easy to embed elsewhere (IDE, LSP, tests).
//!
//! See `ARCHITECTURE.md` §4.

#![allow(dead_code)]

pub mod add_dimension;
pub mod adversarial;
pub mod approver;
pub mod effect_diff;
pub mod meta_verify;
mod dimension_artifact;
mod dimension_registry;
pub mod modules;
pub mod proof_replay;
mod import_integrity;
mod package_conflicts;
mod package_lock;
mod package_manifest;
mod package_metadata;
mod package_policy;
mod package_registry;
mod package_version;
mod native_ability;
mod native_cache;
mod render;
pub mod spec_check;
pub mod spec_site;

pub use add_dimension::{
    add_dimension as install_dimension, add_dimension_with_registry as install_dimension_with_registry,
    AddDimensionOutcome,
};
pub use adversarial::{
    file_github_issues_for_escapes, render_adversarial_prompt, render_adversarial_report,
    run_adversarial_suite, AdversarialAttempt, AdversarialCategory, AdversarialIssueOutcome,
    AdversarialOutcome, AdversarialReport, AdversarialVerdict,
};
pub use package_registry::{
    add_package, publish_package, remove_package, update_package, AddPackageOutcome,
    PackageMutationOutcome, PublishPackageOptions, PublishPackageOutcome,
    RegistryVerificationFailure, RegistryVerificationReport, verify_registry_contract,
};
pub use package_conflicts::{
    render_package_conflict_report, verify_package_lock, PackageConflictFailure,
    PackageConflictKind, PackageConflictReport,
};
pub use package_metadata::{
    package_metadata_from_source, render_package_metadata_markdown, PackageMetadata,
};
pub use approver::{simulate_approver, verify_approver_source};
pub use modules::{
    build_module_resolution, inspect_import_semantics, render_import_semantic_summaries,
    summarize_module_file, ModuleLoadError, NamedModuleSemanticSummary,
};
pub use effect_diff::{
    diff_snapshots, render_effect_diff, snapshot_revision, AgentDiff, AgentSnapshot,
    DimensionChange, EffectDiff, RevisionSnapshot,
};
pub use meta_verify::{
    render_meta_report, verify_counterexample_corpus, Counterexample, MetaKind, MetaVerdict, CORPUS,
};
pub use native_ability::{native_ability, NotNativeReason};
pub use render::{render_all_pretty, render_pretty};
pub use spec_check::{
    extract_spec_examples, render_spec_report, verify_spec_examples, Expectation, SpecExample,
    SpecVerdict, VerdictKind,
};
pub use dimension_artifact::{
    canonical_payload_for_artifact as dimension_artifact_payload,
    verify_dimension_artifact, DimensionArtifactReport,
};
pub use spec_site::{
    build_spec_site, render_spec_site_report, SpecSitePage, SpecSiteReport,
};

// Re-export the runtime + interpreter surface so consumers (CLI, demo
// runner binaries, embedding hosts) only need to depend on the driver.
pub use corvid_runtime::{
    fresh_run_id, load_dotenv_walking, AnthropicAdapter, ApprovalDecision, ApprovalRequest,
    Approver, EnvVarMockAdapter, MockAdapter, OllamaAdapter, OpenAiAdapter, ProgrammaticApprover,
    RedactionSet, Runtime, RuntimeBuilder, RuntimeError, StdinApprover, Tracer,
};
pub use corvid_vm::{build_struct, InterpError, InterpErrorKind, StructValue, Value};

use std::path::Path;

use corvid_ir::IrFile;
pub use corvid_types::{Verdict as LawVerdict, DEFAULT_SAMPLES};


mod build;
mod config_loader;
mod diagnostic;
mod law;
mod pipeline;
mod replay;
mod run;
mod scaffold;
mod eval_runner;
mod test_runner;
mod trace_fresh;
pub use config_loader::{load_corvid_config_for, load_corvid_config_with_path_for};
pub use pipeline::{
    compile, compile_to_abi_with_config, compile_to_ir, compile_to_ir_with_config,
    compile_to_ir_with_config_at_path, compile_with_config, compile_with_config_at_path,
    CompileResult,
};
pub(crate) use pipeline::{lower_driver_file, typecheck_driver_file};
pub use replay::{
    configure_replay_mode, run_replay_from_source, run_replay_from_source_with_builder,
    run_replay_from_source_with_builder_async, ReplayMode, ReplayOutcome,
};
pub use trace_fresh::run_fresh_from_source_async;
pub use build::{
    build_catalog_descriptor_for_source, build_native_to_disk, build_target_to_disk,
    build_server_to_disk, build_to_disk, build_wasm_to_disk, AbiBuildOutput, BuildOutput,
    BuildTarget, NativeBuildOutput, ServerBuildOutput, SigningRequest, TargetBuildOutput,
    WasmBuildOutput,
};
pub use diagnostic::{summarize_diagnostics, Diagnostic};
pub use law::{
    render_dimension_verification_report, render_law_check_report, run_dimension_verification,
    run_law_checks, DimensionVerificationReport,
};
pub use proof_replay::{replay_dimension_proof, ProofReplayResult, ProofReplayStatus};
pub use run::{
    build_or_get_cached_native, run_native, run_with_target, CachedNativeBinary, RunError,
    RunTarget,
};
pub use scaffold::{scaffold_new, scaffold_new_in};
pub use eval_runner::{
    default_eval_options, render_eval_report, run_evals_at_path, run_evals_at_path_with_options,
    CorvidEvalReport, EvalRunnerError,
};
pub use test_runner::{
    render_test_report, run_tests_at_path, run_tests_at_path_with_options, test_options,
    CorvidTestReport, TestRunnerError,
};






/// Compile a `.cor` file and run the chosen agent against `runtime`.
///
/// `agent` selects which agent to invoke. Pass `None` to run the file's
/// only agent (errors if there's more than one). `args` are passed as
/// the agent's parameters; pass an empty vec for parameter-less agents.
pub async fn run_with_runtime(
    path: &Path,
    agent: Option<&str>,
    args: Vec<Value>,
    runtime: &Runtime,
) -> Result<Value, RunError> {
    let source = std::fs::read_to_string(path).map_err(|e| RunError::Io {
        path: path.to_path_buf(),
        error: e,
    })?;
    let config = load_corvid_config_for(path);
    let ir = compile_to_ir_with_config_at_path(&source, path, config.as_ref())
        .map_err(RunError::Compile)?;
    run_ir_with_runtime(&ir, agent, args, runtime).await
}

/// Like `run_with_runtime`, but takes already-lowered IR. Useful for
/// embedding hosts that compile once and run many times.
pub async fn run_ir_with_runtime(
    ir: &IrFile,
    agent: Option<&str>,
    args: Vec<Value>,
    runtime: &Runtime,
) -> Result<Value, RunError> {
    if ir.agents.is_empty() {
        return Err(RunError::NoAgents);
    }
    let chosen_name = match agent {
        Some(name) => {
            if !ir.agents.iter().any(|a| a.name == name) {
                return Err(RunError::UnknownAgent {
                    name: name.to_string(),
                    available: ir.agents.iter().map(|a| a.name.clone()).collect(),
                });
            }
            name.to_string()
        }
        None => {
            if ir.agents.len() == 1 {
                ir.agents[0].name.clone()
            } else {
                // Prefer an agent named `main` if one exists.
                if let Some(main) = ir.agents.iter().find(|a| a.name == "main") {
                    main.name.clone()
                } else {
                    return Err(RunError::AmbiguousAgent {
                        available: ir.agents.iter().map(|a| a.name.clone()).collect(),
                    });
                }
            }
        }
    };
    let chosen = ir
        .agents
        .iter()
        .find(|a| a.name == chosen_name)
        .expect("agent presence checked above");
    if args.is_empty() && !chosen.params.is_empty() {
        return Err(RunError::NeedsArgs {
            agent: chosen.name.clone(),
            expected: chosen.params.len(),
        });
    }
    corvid_vm::run_agent(ir, &chosen.name, args, runtime)
        .await
        .map_err(RunError::Interp)
}

#[cfg(test)]
mod tests;
