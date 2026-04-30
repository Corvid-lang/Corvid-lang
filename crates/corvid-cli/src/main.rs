//! The `corvid` CLI.
//!
//! Subcommands:
//!   corvid new <name>         scaffold a new project
//!   corvid check <file>       type-check a source file
//!   corvid build <file>       compile to target/py/<name>.py
//!   corvid build --target=wasm <file> emits browser/edge artifacts
//!   corvid run <file>         build + invoke python on the output
//!   corvid repl               start the interactive REPL
//!   corvid test <file>        run Corvid test declarations in a source file
//!   corvid test <what>        run verification suites (dimensions, spec, rewrites, adversarial)
//!   corvid verify             cross-tier effect-profile verification
//!   corvid effect-diff        diff composed effect profiles between two revisions
//!   corvid add                add a package dependency to Corvid.lock
//!   corvid remove             remove a package dependency from corvid.toml and Corvid.lock
//!   corvid update             refresh a package dependency through its registry
//!   corvid add-dimension      install a dimension from the effect registry
//!   corvid routing-report     aggregate dispatch traces into routing guidance
//!   corvid cost-frontier      compute prompt cost/quality Pareto frontier
//!   corvid tour               open runnable demos for Corvid inventions
//!   corvid import-summary     inspect imported module semantic contracts
//!   corvid eval --swap-model <id> <trace>  retrospective model migration analysis
//!   corvid replay <trace>     re-execute a recorded trace deterministically
//!   corvid replay --model <id> <trace>  differential replay against a different model
//!   corvid abi dump <lib>     inspect the embedded ABI/capability catalog
//!   corvid trace list         list traces under target/trace/
//!   corvid trace show <id>    print a recorded trace as formatted JSON
//!   corvid trace dag <id>     render provenance DAG as Graphviz DOT
//!   corvid claim --explain <lib>  explain the guarantees claimed by a cdylib

mod abi_cmd;
mod approver_cmd;
mod audit_cmd;
mod approvals_cmd;
mod auth_cmd;
mod build_cmd;
mod cli;
mod commands;
mod doctor_cmd;
mod format;
mod migrate_cmd;
mod package_cmd;
mod run_cmd;
mod verify_cmd;

use build_cmd::cmd_build;
use cli::jobs::*;
use cli::migrate::*;
use cli::observe::*;
use cli::package::*;
use cli::root::*;
use commands::eval::*;
use commands::jobs::*;
use commands::misc::*;
use commands::test::*;
use doctor_cmd::cmd_doctor_v2;
use migrate_cmd::{cmd_migrate, cmd_migrate_down};
use package_cmd::{
    cmd_add_package, cmd_package_metadata, cmd_package_publish, cmd_package_verify_lock,
    cmd_package_verify_registry, cmd_remove_package, cmd_update_package,
};
use run_cmd::cmd_run;
use verify_cmd::cmd_verify;
use format::{
    approval_summary_value, approvals_inspect_summary, approvals_queue_summary,
    audit_event_value,
};
mod bench_cmd;
mod bind_cmd;
mod bundle_cmd;
mod capsule_cmd;
mod claim_cmd;
mod connectors_cmd;
mod contract_cmd;
mod cost_frontier;
mod deploy_cmd;
mod eval_cmd;
mod lineage_cmd;
mod observe_cmd;
mod observe_helpers_cmd;
mod receipt_cache;
mod receipt_cmd;
mod release_cmd;
mod replay;
mod routing_report;
mod test_from_traces;
mod tour;
mod trace_cmd;
mod trace_dag;
mod trace_diff;
mod upgrade_cmd;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;

#[allow(unused_imports)]
use corvid_driver::{
    build_native_to_disk, build_server_to_disk, build_spec_site, build_target_to_disk,
    build_to_disk, build_wasm_to_disk, compile, compile_with_config, diff_snapshots,
    file_github_issues_for_escapes, inspect_import_semantics, load_corvid_config_for,
    load_corvid_config_with_path_for, load_dotenv_walking, render_adversarial_report,
    render_all_pretty, render_dimension_verification_report, render_effect_diff,
    render_import_semantic_summaries, render_law_check_report, render_spec_report,
    render_spec_site_report, render_test_report, run_adversarial_suite, run_dimension_verification,
    run_law_checks, run_native, run_tests_at_path_with_options, run_with_target, scaffold_new,
    snapshot_revision, test_options, verify_spec_examples, BuildTarget, RunTarget, VerdictKind,
    DEFAULT_SAMPLES,
};


fn main() -> ExitCode {
    match std::thread::Builder::new()
        .name("corvid-cli".to_string())
        .stack_size(16 * 1024 * 1024)
        .spawn(main_impl)
    {
        Ok(handle) => handle.join().unwrap_or_else(|_| {
            eprintln!("error: corvid CLI worker thread panicked");
            ExitCode::from(101)
        }),
        Err(err) => {
            eprintln!("error: failed to start corvid CLI worker thread: {err}");
            ExitCode::from(2)
        }
    }
}

fn main_impl() -> ExitCode {
    let cli = Cli::parse();

    let result = match cli.command {
        Some(Command::New { name }) => cmd_new(&name),
        Some(Command::Check { file }) => cmd_check(&file),
        Some(Command::Build {
            file,
            target,
            with_tools_lib,
            header,
            abi_descriptor,
            all_artifacts,
            sign,
            key_id,
        }) => cmd_build(
            &file,
            &target,
            with_tools_lib.as_deref(),
            header,
            abi_descriptor,
            all_artifacts,
            sign.as_deref(),
            key_id.as_deref(),
        ),
        Some(Command::Run {
            file,
            target,
            with_tools_lib,
        }) => cmd_run(&file, &target, with_tools_lib.as_deref()),
        Some(Command::Test {
            target,
            meta,
            site_out,
            count,
            model,
            update_snapshots,
            from_traces,
            from_traces_source,
            replay_model,
            only_dangerous,
            only_prompt,
            only_tool,
            since,
            promote,
            flake_detect,
        }) => {
            if let Some(dir) = from_traces {
                test_from_traces::run_test_from_traces(test_from_traces::TestFromTracesArgs {
                    trace_dir: &dir,
                    source: from_traces_source.as_deref(),
                    replay_model: replay_model.as_deref(),
                    only_dangerous,
                    only_prompt: only_prompt.as_deref(),
                    only_tool: only_tool.as_deref(),
                    since: since.as_deref(),
                    promote,
                    flake_detect,
                })
            } else {
                cmd_test(
                    target.as_deref(),
                    meta,
                    site_out.as_deref(),
                    count,
                    &model,
                    update_snapshots,
                )
            }
        }
        Some(Command::Verify {
            corpus,
            shrink,
            json,
        }) => cmd_verify(corpus.as_deref(), shrink.as_deref(), json),
        Some(Command::EffectDiff { before, after }) => cmd_effect_diff(&before, &after),
        Some(Command::AddDimension { spec, registry }) => {
            cmd_add_dimension(&spec, registry.as_deref())
        }
        Some(Command::Add { spec, registry }) => cmd_add_package(&spec, registry.as_deref()),
        Some(Command::Remove { name }) => cmd_remove_package(&name),
        Some(Command::Update { spec, registry }) => cmd_update_package(&spec, registry.as_deref()),
        Some(Command::RoutingReport {
            since,
            since_commit,
            json,
            trace_dir,
        }) => cmd_routing_report(
            trace_dir.as_deref(),
            since.as_deref(),
            since_commit.as_deref(),
            json,
        ),
        Some(Command::CostFrontier {
            prompt,
            since,
            since_commit,
            json,
            trace_dir,
        }) => cmd_cost_frontier(
            &prompt,
            trace_dir.as_deref(),
            since.as_deref(),
            since_commit.as_deref(),
            json,
        ),
        Some(Command::Tour { list, topic }) => tour::cmd_tour(list, topic.as_deref()),
        Some(Command::ImportSummary { file, json }) => cmd_import_summary(&file, json),
        Some(Command::Eval {
            inputs,
            source,
            swap_model,
            max_spend,
            golden_traces,
            promote_out,
        }) => eval_cmd::run_eval(
            &inputs,
            source.as_deref(),
            swap_model.as_deref(),
            max_spend,
            golden_traces.as_deref(),
            promote_out.as_deref(),
        ),
        Some(Command::EvalDrift {
            baseline,
            candidate,
            explain,
        }) => cmd_eval_drift(baseline, candidate, explain),
        Some(Command::EvalFromFeedback {
            feedback,
            trace_dir,
            out,
        }) => cmd_eval_from_feedback(feedback, trace_dir, out),
        Some(Command::Replay {
            trace,
            source,
            model,
            mutate,
        }) => replay::run_replay(
            &trace,
            source.as_deref(),
            model.as_deref(),
            mutate.as_deref(),
        ),
        Some(Command::Abi { command }) => match command {
            AbiCommand::Dump { library } => abi_cmd::run_dump(&library),
            AbiCommand::Hash { source } => abi_cmd::run_hash(&source),
            AbiCommand::Verify {
                library,
                expected_hash,
            } => abi_cmd::run_verify(&library, &expected_hash),
        },
        Some(Command::Bind {
            language,
            descriptor,
            out,
        }) => bind_cmd::run_bind(&language, &descriptor, &out),
        Some(Command::Bundle { command }) => match command {
            BundleCommand::Verify { path, rebuild } => bundle_cmd::run_verify(&path, rebuild),
            BundleCommand::Diff { old, new, json } => bundle_cmd::run_diff(&old, &new, json),
            BundleCommand::Audit {
                path,
                question,
                json,
            } => bundle_cmd::run_audit(&path, question.as_deref(), json),
            BundleCommand::Explain { path, json } => bundle_cmd::run_explain(&path, json),
            BundleCommand::Report { path, format, json } => {
                bundle_cmd::run_report(&path, &format, json)
            }
            BundleCommand::Query {
                path,
                delta,
                predecessor,
                json,
            } => bundle_cmd::run_query(&path, &delta, predecessor.as_deref(), json),
            BundleCommand::Lineage { path, json } => bundle_cmd::run_lineage(&path, json),
        },
        Some(Command::Approver { command }) => match command {
            ApproverCommand::Check {
                approver,
                max_budget_usd,
            } => approver_cmd::run_check(&approver, max_budget_usd),
            ApproverCommand::Simulate {
                approver,
                site_label,
                args,
                max_budget_usd,
            } => approver_cmd::run_simulate(&approver, &site_label, &args, max_budget_usd),
            ApproverCommand::Card {
                site_label,
                args,
                format,
            } => approver_cmd::run_card(&site_label, &args, format),
        },
        Some(Command::Capsule { command }) => match command {
            CapsuleCommand::Create { trace, cdylib, out } => {
                capsule_cmd::run_create(&trace, &cdylib, out.as_deref())
            }
            CapsuleCommand::Replay { capsule } => capsule_cmd::run_replay(&capsule),
        },
        Some(Command::Trace { command }) => match command {
            TraceCommand::List { trace_dir } => trace_cmd::run_list(trace_dir.as_deref()),
            TraceCommand::Show {
                id_or_path,
                trace_dir,
            } => trace_cmd::run_show(&id_or_path, trace_dir.as_deref()),
            TraceCommand::Dag {
                id_or_path,
                trace_dir,
            } => trace_dag::run_dag(&id_or_path, trace_dir.as_deref()),
            TraceCommand::Lineage {
                id_or_path,
                trace_dir,
            } => lineage_cmd::run_lineage(&id_or_path, trace_dir.as_deref()),
        },
        Some(Command::Observe { command }) => match command {
            ObserveCommand::List { trace_dir } => observe_cmd::run_list(trace_dir.as_deref()),
            ObserveCommand::Show {
                id_or_path,
                trace_dir,
            } => observe_cmd::run_show(&id_or_path, trace_dir.as_deref()),
            ObserveCommand::Drift {
                baseline,
                candidate,
                json,
            } => observe_cmd::run_drift(&baseline, &candidate, json),
            ObserveCommand::Explain {
                trace_id,
                trace_dir,
            } => cmd_observe_explain(trace_id, trace_dir),
            ObserveCommand::CostOptimise {
                agent,
                trace_dir,
                top_n,
            } => cmd_observe_cost_optimise(agent, trace_dir, top_n),
        },
        Some(Command::TraceDiff {
            base_sha,
            head_sha,
            path,
            traces,
            narrative,
            format,
            sign,
            sign_key_id,
            policy,
            stack,
            no_replay_skip,
        }) => {
            let parsed = narrative
                .parse::<trace_diff::NarrativeMode>()
                .map_err(anyhow::Error::msg)
                .and_then(|narrative_mode| {
                    trace_diff::OutputFormat::parse(&format)
                        .map_err(anyhow::Error::msg)
                        .map(|format| (narrative_mode, format))
                })
                .and_then(|(narrative_mode, format)| {
                    stack
                        .as_deref()
                        .map(trace_diff::parse_stack_spec)
                        .transpose()
                        .map_err(anyhow::Error::msg)
                        .map(|stack_spec| (narrative_mode, format, stack_spec))
                });
            match parsed {
                Ok((narrative_mode, format, stack_spec)) => {
                    trace_diff::run_trace_diff(trace_diff::TraceDiffArgs {
                        base_sha: &base_sha,
                        head_sha: &head_sha,
                        source_path: &path,
                        trace_dir: traces.as_deref(),
                        narrative_mode,
                        format,
                        sign_key_path: sign.as_deref(),
                        sign_key_id: sign_key_id.as_deref(),
                        policy_path: policy.as_deref(),
                        stack_spec,
                        no_replay_skip,
                    })
                }
                Err(e) => Err(e),
            }
        }
        Some(Command::Receipt { command }) => match command {
            ReceiptCommand::Show { hash } => receipt_cmd::run_show(&hash),
            ReceiptCommand::Verify { envelope, key } => receipt_cmd::run_verify(&envelope, &key),
            ReceiptCommand::VerifyAbi { cdylib, key } => receipt_cmd::run_verify_abi(&cdylib, &key),
        },
        Some(Command::Package { command }) => match command {
            PackageCommand::Metadata {
                source,
                name,
                version,
                signature,
                json,
            } => cmd_package_metadata(&source, &name, &version, signature.as_deref(), json),
            PackageCommand::VerifyRegistry { registry, json } => {
                cmd_package_verify_registry(&registry, json)
            }
            PackageCommand::VerifyLock { json } => cmd_package_verify_lock(json),
            PackageCommand::Publish {
                source,
                name,
                version,
                out,
                url_base,
                key,
                key_id,
            } => cmd_package_publish(&source, &name, &version, &out, &url_base, &key, &key_id),
        },
        Some(Command::Claim {
            command,
            explain,
            cdylib,
            key,
            source,
        }) => match command {
            Some(ClaimCommand::Audit { inventory, json }) => {
                claim_cmd::run_claim_audit(&inventory, json)
            }
            None => {
                if let Some(cdylib) = cdylib {
                    claim_cmd::run_claim_explain(&cdylib, explain, key.as_deref(), source.as_deref())
                } else {
                    Err(anyhow::anyhow!(
                        "`corvid claim --explain` requires a cdylib path"
                    ))
                }
            }
        },
        Some(Command::Repl) => cmd_repl(),
        Some(Command::Doctor) => cmd_doctor_v2(),
        Some(Command::Audit { file, json }) => audit_cmd::run_audit(&file, json),
        Some(Command::Deploy { command }) => match command {
            DeployCommand::Package { app, out } => {
                let out = out.unwrap_or_else(|| app.join("target").join("deploy-package"));
                deploy_cmd::run_package(&app, &out).map(|_| 0)
            }
            DeployCommand::Compose { app, out } => {
                let out = out.unwrap_or_else(|| app.join("target").join("compose"));
                deploy_cmd::run_compose(&app, &out).map(|_| 0)
            }
            DeployCommand::Paas { app, out } => {
                let out = out.unwrap_or_else(|| app.join("target").join("paas"));
                deploy_cmd::run_paas(&app, &out).map(|_| 0)
            }
            DeployCommand::K8s { app, out } => {
                let out = out.unwrap_or_else(|| app.join("target").join("k8s"));
                deploy_cmd::run_k8s(&app, &out).map(|_| 0)
            }
            DeployCommand::Systemd { app, out } => {
                let out = out.unwrap_or_else(|| app.join("target").join("systemd"));
                deploy_cmd::run_systemd(&app, &out).map(|_| 0)
            }
        },
        Some(Command::Release {
            channel,
            version,
            out,
        }) => {
            let out = out.unwrap_or_else(|| {
                PathBuf::from("target")
                    .join("release")
                    .join(channel.as_str())
            });
            release_cmd::run_release(&channel, version.as_deref(), &out).map(|_| 0)
        }
        Some(Command::Upgrade { command }) => match command {
            UpgradeCommand::Check { path, json } => upgrade_cmd::run_check(&path, json),
            UpgradeCommand::Apply { path, json } => upgrade_cmd::run_apply(&path, json),
        },
        Some(Command::Migrate { command }) => match command {
            MigrateCommand::Status {
                dir,
                state,
                database,
                dry_run,
            } => cmd_migrate("status", &dir, &state, &database, dry_run),
            MigrateCommand::Up {
                dir,
                state,
                database,
                dry_run,
            } => cmd_migrate("up", &dir, &state, &database, dry_run),
            MigrateCommand::Down {
                dir,
                down_dir,
                state,
                database,
                dry_run,
            } => cmd_migrate_down(&dir, &down_dir, &state, &database, dry_run),
        },
        Some(Command::Jobs { command }) => match command {
            JobsCommand::Enqueue {
                state,
                task,
                payload,
                input_schema,
                max_retries,
                budget_usd,
                effect_summary,
                replay_key,
                idempotency_key,
                delay_ms,
            } => cmd_jobs_enqueue(
                &state,
                &task,
                &payload,
                input_schema,
                max_retries,
                budget_usd,
                effect_summary,
                replay_key,
                idempotency_key,
                delay_ms,
            ),
            JobsCommand::RunOne {
                state,
                output_kind,
                output_fingerprint,
                fail_kind,
                fail_fingerprint,
                retry_base_ms,
            } => cmd_jobs_run_one(
                &state,
                output_kind,
                output_fingerprint,
                fail_kind,
                fail_fingerprint,
                retry_base_ms,
            ),
            JobsCommand::Run {
                state,
                workers,
                lease_ttl_ms,
                idle_poll_ms,
                max_runtime_ms,
            } => cmd_jobs_run(
                &state,
                workers,
                lease_ttl_ms,
                idle_poll_ms,
                max_runtime_ms,
            ),
            JobsCommand::Inspect { state, job } => cmd_jobs_inspect(&state, &job),
            JobsCommand::Retry { state, job } => cmd_jobs_retry(&state, &job),
            JobsCommand::Cancel { state, job } => cmd_jobs_cancel(&state, &job),
            JobsCommand::Pause { state, reason } => cmd_jobs_pause(&state, reason.as_deref()),
            JobsCommand::Resume { state } => cmd_jobs_resume(&state),
            JobsCommand::Drain { state, reason } => cmd_jobs_drain(&state, reason.as_deref()),
            JobsCommand::ExportTrace { state, job, out } => {
                cmd_jobs_export_trace(&state, &job, out.as_deref())
            }
            JobsCommand::WaitApproval {
                state,
                worker_id,
                lease_ttl_ms,
                approval_id,
                approval_expires_ms,
                approval_reason,
            } => cmd_jobs_wait_approval(
                &state,
                &worker_id,
                lease_ttl_ms,
                &approval_id,
                approval_expires_ms,
                &approval_reason,
            ),
            JobsCommand::Approvals { state } => cmd_jobs_approvals(&state),
            JobsCommand::Approval { command } => match command {
                JobsApprovalCommand::Decide {
                    state,
                    job,
                    approval_id,
                    decision,
                    actor,
                    reason,
                } => cmd_jobs_approval_decide(&state, &job, &approval_id, decision, &actor, reason),
                JobsApprovalCommand::Audit { state, job } => cmd_jobs_approval_audit(&state, &job),
            },
            JobsCommand::Loop { command } => match command {
                JobsLoopCommand::Limits {
                    state,
                    job,
                    max_steps,
                    max_wall_ms,
                    max_spend_usd,
                    max_tool_calls,
                } => cmd_jobs_loop_limits(
                    &state,
                    &job,
                    max_steps,
                    max_wall_ms,
                    max_spend_usd,
                    max_tool_calls,
                ),
                JobsLoopCommand::Record {
                    state,
                    job,
                    steps,
                    wall_ms,
                    spend_usd,
                    tool_calls,
                    actor,
                } => cmd_jobs_loop_record(
                    &state, &job, steps, wall_ms, spend_usd, tool_calls, &actor,
                ),
                JobsLoopCommand::Usage { state, job } => cmd_jobs_loop_usage(&state, &job),
                JobsLoopCommand::Heartbeat {
                    state,
                    job,
                    actor,
                    message,
                } => cmd_jobs_loop_heartbeat(&state, &job, &actor, message),
                JobsLoopCommand::StallPolicy {
                    state,
                    job,
                    stall_after_ms,
                    action,
                } => cmd_jobs_loop_stall_policy(&state, &job, stall_after_ms, action),
                JobsLoopCommand::CheckStall { state, job, actor } => {
                    cmd_jobs_loop_check_stall(&state, &job, &actor)
                }
            },
            JobsCommand::Schedule { command } => match command {
                JobsScheduleCommand::Add {
                    state,
                    id,
                    cron,
                    zone,
                    task,
                    payload,
                    max_retries,
                    budget_usd,
                    effect_summary,
                    replay_key_prefix,
                    missed_policy,
                } => cmd_jobs_schedule_add(
                    &state,
                    &id,
                    &cron,
                    &zone,
                    &task,
                    &payload,
                    max_retries,
                    budget_usd,
                    effect_summary,
                    replay_key_prefix,
                    missed_policy,
                ),
                JobsScheduleCommand::List { state } => cmd_jobs_schedule_list(&state),
                JobsScheduleCommand::Recover {
                    state,
                    max_missed_per_schedule,
                } => cmd_jobs_schedule_recover(&state, max_missed_per_schedule),
            },
            JobsCommand::Limit { command } => match command {
                JobsLimitCommand::Set {
                    state,
                    scope,
                    task,
                    max_leased,
                } => cmd_jobs_limit_set(&state, scope, task.as_deref(), max_leased),
                JobsLimitCommand::List { state } => cmd_jobs_limit_list(&state),
            },
            JobsCommand::Checkpoint { command } => match command {
                JobsCheckpointCommand::Add {
                    state,
                    job,
                    kind,
                    label,
                    payload,
                    payload_fingerprint,
                } => cmd_jobs_checkpoint_add(
                    &state,
                    &job,
                    kind,
                    &label,
                    &payload,
                    payload_fingerprint,
                ),
                JobsCheckpointCommand::List { state, job } => {
                    cmd_jobs_checkpoint_list(&state, &job)
                }
                JobsCheckpointCommand::Resume { state, job } => {
                    cmd_jobs_checkpoint_resume(&state, &job)
                }
            },
            JobsCommand::Dlq { state } => cmd_jobs_dlq(&state),
        },
        Some(Command::Bench { command }) => match command {
            BenchCommand::Compare {
                target,
                session,
                json,
            } => bench_cmd::run_compare(&target, &session, json),
        },
        Some(Command::Contract { command }) => match command {
            ContractCommand::List { json, class, kind } => {
                contract_cmd::run_list(json, class.as_deref(), kind.as_deref())
            }
            ContractCommand::RegenDoc { output } => contract_cmd::run_regen_doc(&output),
        },
        Some(Command::Connectors { command }) => cmd_connectors(command),
        Some(Command::Auth { command }) => cmd_auth(command),
        Some(Command::Approvals { command }) => cmd_approvals(command),
        None => {
            println!("corvid — the AI-native language compiler");
            println!("Run `corvid --help` for usage.");
            Ok(0)
        }
    };

    match result {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(2)
        }
    }
}

// ------------------------------------------------------------
// Commands
// ------------------------------------------------------------


fn cmd_connectors(command: ConnectorsCommand) -> Result<u8> {
    match command {
        ConnectorsCommand::List { json } => {
            let entries = connectors_cmd::run_list()?;
            if json {
                let out = serde_json::to_string_pretty(&entries.iter().map(|e| {
                    serde_json::json!({
                        "name": e.name,
                        "provider": e.provider,
                        "modes": e.modes,
                        "scope_count": e.scope_count,
                        "write_scopes": e.write_scopes,
                        "rate_limit": e.rate_limit_summary,
                        "redaction_count": e.redaction_count,
                    })
                }).collect::<Vec<_>>())?;
                println!("{out}");
            } else {
                println!("{:<10} {:<14} {:<22} {:<6} {:<28} {}",
                    "NAME", "PROVIDER", "MODES", "SCOPES", "RATE LIMIT", "WRITE SCOPES");
                for e in &entries {
                    println!("{:<10} {:<14} {:<22} {:<6} {:<28} {}",
                        e.name,
                        e.provider,
                        e.modes.join(","),
                        e.scope_count,
                        if e.rate_limit_summary.len() > 27 {
                            format!("{}…", &e.rate_limit_summary[..26])
                        } else {
                            e.rate_limit_summary.clone()
                        },
                        e.write_scopes.join(","),
                    );
                }
            }
            Ok(0)
        }
        ConnectorsCommand::Check { live, json } => {
            let entries = connectors_cmd::run_check(live)?;
            let any_invalid = entries.iter().any(|e| !e.valid);
            if json {
                let out = serde_json::to_string_pretty(&entries.iter().map(|e| {
                    serde_json::json!({
                        "name": e.name,
                        "valid": e.valid,
                        "diagnostics": e.diagnostics,
                    })
                }).collect::<Vec<_>>())?;
                println!("{out}");
            } else {
                println!("{:<12} {:<7} DIAGNOSTICS", "NAME", "VALID");
                for e in &entries {
                    let status = if e.valid { "✓" } else { "✗" };
                    println!("{:<12} {:<7} {}", e.name, status, e.diagnostics.join("; "));
                }
            }
            Ok(if any_invalid { 1 } else { 0 })
        }
        ConnectorsCommand::Run {
            connector,
            operation,
            scope,
            mode,
            payload,
            mock,
            approval_id,
            replay_key,
            tenant_id,
            actor_id,
            token_id,
            now_ms,
        } => {
            let payload_value = match payload {
                Some(path) => {
                    let raw = std::fs::read_to_string(&path)
                        .with_context(|| format!("reading payload from `{}`", path.display()))?;
                    Some(serde_json::from_str(&raw).with_context(|| "payload is not JSON")?)
                }
                None => None,
            };
            let mock_value = match mock {
                Some(path) => {
                    let raw = std::fs::read_to_string(&path)
                        .with_context(|| format!("reading mock from `{}`", path.display()))?;
                    Some(serde_json::from_str(&raw).with_context(|| "mock is not JSON")?)
                }
                None => None,
            };
            let resolved_now_ms = now_ms.unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0)
            });
            let output = connectors_cmd::run_run(connectors_cmd::ConnectorRunArgs {
                connector,
                operation,
                scope_id: scope,
                mode,
                payload: payload_value,
                mock_payload: mock_value,
                approval_id,
                replay_key,
                tenant_id,
                actor_id,
                token_id,
                now_ms: resolved_now_ms,
            })?;
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                "connector": output.connector,
                "operation": output.operation,
                "mode": output.mode,
                "payload": output.payload,
            }))?);
            Ok(0)
        }
        ConnectorsCommand::Oauth { command } => match command {
            ConnectorsOauthCommand::Init {
                provider,
                client_id,
                redirect_uri,
                scope,
            } => {
                let output = connectors_cmd::run_oauth_init(connectors_cmd::OauthInitArgs {
                    provider,
                    client_id,
                    redirect_uri,
                    scopes: scope,
                })?;
                println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                    "provider": output.provider,
                    "state": output.state,
                    "code_verifier": output.code_verifier,
                    "code_challenge": output.code_challenge,
                    "authorization_url": output.authorization_url,
                }))?);
                Ok(0)
            }
            ConnectorsOauthCommand::Rotate {
                provider,
                token_id,
                access_token,
                refresh_token,
                client_id,
                client_secret,
            } => {
                let output = connectors_cmd::run_oauth_rotate(connectors_cmd::OauthRotateArgs {
                    provider,
                    token_id,
                    access_token,
                    refresh_token,
                    client_id,
                    client_secret,
                })?;
                println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                    "provider": output.provider,
                    "access_token": output.access_token,
                    "refresh_token": output.refresh_token,
                    "expires_at_ms": output.expires_at_ms,
                }))?);
                Ok(0)
            }
        },
        ConnectorsCommand::VerifyWebhook {
            signature,
            secret_env,
            body_file,
            provider,
            headers,
        } => {
            let parsed_headers = headers
                .iter()
                .map(|h| {
                    let mut parts = h.splitn(2, '=');
                    let name = parts.next().unwrap_or_default().to_string();
                    let value = parts.next().unwrap_or_default().to_string();
                    (name, value)
                })
                .collect::<Vec<_>>();
            let output = connectors_cmd::run_verify_webhook(connectors_cmd::WebhookVerifyArgs {
                signature,
                secret_env,
                body_file,
                provider,
                headers: parsed_headers,
            })?;
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                "valid": output.valid,
                "algorithm": output.algorithm,
                "outcome": output.outcome,
            }))?);
            Ok(if output.valid { 0 } else { 1 })
        }
    }
}

fn cmd_auth(command: AuthCommand) -> Result<u8> {
    match command {
        AuthCommand::Migrate {
            auth_state,
            approvals_state,
        } => {
            let out = auth_cmd::run_auth_migrate(auth_cmd::AuthMigrateArgs {
                auth_state,
                approvals_state,
            })?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "auth_state": out.auth_state,
                    "approvals_state": out.approvals_state,
                    "auth_initialised": out.auth_initialised,
                    "approvals_initialised": out.approvals_initialised,
                }))?
            );
            Ok(0)
        }
        AuthCommand::Keys { command } => match command {
            AuthKeysCommand::Issue {
                auth_state,
                key_id,
                service_actor,
                tenant,
                raw_key,
                scope_fingerprint,
                display_name,
                expires_at_ms,
            } => {
                let out = auth_cmd::run_auth_key_issue(auth_cmd::AuthKeyIssueArgs {
                    auth_state,
                    key_id,
                    service_actor_id: service_actor,
                    tenant_id: tenant,
                    raw_key,
                    scope_fingerprint,
                    display_name,
                    expires_at_ms: expires_at_ms.unwrap_or(u64::MAX),
                })?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "key_id": out.key_id,
                        "service_actor_id": out.service_actor_id,
                        "tenant_id": out.tenant_id,
                        "key_hash_prefix": out.key_hash_prefix,
                        "scope_fingerprint": out.scope_fingerprint,
                        "expires_at_ms": out.expires_at_ms,
                        "raw_key": out.raw_key,
                    }))?
                );
                Ok(0)
            }
            AuthKeysCommand::Revoke {
                auth_state,
                key_id,
            } => {
                let out = auth_cmd::run_auth_key_revoke(auth_cmd::AuthKeyRevokeArgs {
                    auth_state,
                    key_id,
                })?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "key_id": out.key_id,
                        "revoked_at_ms": out.revoked_at_ms,
                    }))?
                );
                Ok(0)
            }
            AuthKeysCommand::Rotate {
                auth_state,
                key_id,
                service_actor,
                tenant,
                new_key_id,
                new_raw_key,
                expires_at_ms,
            } => {
                let out = auth_cmd::run_auth_key_rotate(auth_cmd::AuthKeyRotateArgs {
                    auth_state,
                    key_id,
                    service_actor_id: service_actor,
                    tenant_id: tenant,
                    new_key_id,
                    new_raw_key,
                    expires_at_ms: expires_at_ms.unwrap_or(u64::MAX),
                })?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "revoked_key_id": out.revoked_key_id,
                        "new_key_id": out.new_key_id,
                        "raw_key": out.raw_key,
                        "scope_fingerprint": out.scope_fingerprint,
                        "expires_at_ms": out.expires_at_ms,
                    }))?
                );
                Ok(0)
            }
        },
    }
}

fn cmd_approvals(command: ApprovalsCommand) -> Result<u8> {
    use approvals_cmd::*;
    match command {
        ApprovalsCommand::Queue {
            approvals_state,
            tenant,
            status,
        } => {
            let out = run_approvals_queue(ApprovalsQueueArgs {
                approvals_state,
                tenant_id: tenant,
                status,
            })?;
            println!("{}", serde_json::to_string_pretty(&serde_json::to_value(approvals_queue_summary(&out))?)?);
            Ok(0)
        }
        ApprovalsCommand::Inspect {
            approvals_state,
            tenant,
            approval_id,
        } => {
            let out = run_approvals_inspect(ApprovalsInspectArgs {
                approvals_state,
                tenant_id: tenant,
                approval_id,
            })?;
            println!("{}", serde_json::to_string_pretty(&approvals_inspect_summary(&out))?);
            Ok(0)
        }
        ApprovalsCommand::Approve {
            approvals_state,
            tenant,
            actor,
            role,
            reason,
            approval_id,
        } => {
            let summary = run_approvals_approve(ApprovalsTransitionArgs {
                approvals_state,
                tenant_id: tenant,
                approval_id,
                actor_id: actor,
                role,
                reason,
            })?;
            println!("{}", serde_json::to_string_pretty(&approval_summary_value(&summary))?);
            Ok(0)
        }
        ApprovalsCommand::Deny {
            approvals_state,
            tenant,
            actor,
            role,
            reason,
            approval_id,
        } => {
            let summary = run_approvals_deny(ApprovalsTransitionArgs {
                approvals_state,
                tenant_id: tenant,
                approval_id,
                actor_id: actor,
                role,
                reason,
            })?;
            println!("{}", serde_json::to_string_pretty(&approval_summary_value(&summary))?);
            Ok(0)
        }
        ApprovalsCommand::Expire {
            approvals_state,
            tenant,
            actor,
            reason,
            approval_id,
        } => {
            let summary = run_approvals_expire(ApprovalsExpireArgs {
                approvals_state,
                tenant_id: tenant,
                approval_id,
                actor_id: actor,
                reason,
            })?;
            println!("{}", serde_json::to_string_pretty(&approval_summary_value(&summary))?);
            Ok(0)
        }
        ApprovalsCommand::Comment {
            approvals_state,
            tenant,
            actor,
            comment,
            approval_id,
        } => {
            let event = run_approvals_comment(ApprovalsCommentArgs {
                approvals_state,
                tenant_id: tenant,
                approval_id,
                actor_id: actor,
                comment,
            })?;
            println!("{}", serde_json::to_string_pretty(&audit_event_value(&event))?);
            Ok(0)
        }
        ApprovalsCommand::Delegate {
            approvals_state,
            tenant,
            actor,
            role,
            delegate_to,
            reason,
            approval_id,
        } => {
            let summary = run_approvals_delegate(ApprovalsDelegateArgs {
                approvals_state,
                tenant_id: tenant,
                approval_id,
                actor_id: actor,
                role,
                delegate_to,
                reason,
            })?;
            println!("{}", serde_json::to_string_pretty(&approval_summary_value(&summary))?);
            Ok(0)
        }
        ApprovalsCommand::Batch {
            approvals_state,
            tenant,
            actor,
            role,
            reason,
            ids,
        } => {
            let out = run_approvals_batch(ApprovalsBatchArgs {
                approvals_state,
                tenant_id: tenant,
                actor_id: actor,
                role,
                approval_ids: ids,
                reason,
            })?;
            let approved = out
                .approved
                .iter()
                .map(approval_summary_value)
                .collect::<Vec<_>>();
            let failed = out
                .failed
                .iter()
                .map(|f| serde_json::json!({"approval_id": f.approval_id, "reason": f.reason}))
                .collect::<Vec<_>>();
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "approved": approved,
                    "failed": failed,
                }))?
            );
            Ok(if out.failed.is_empty() { 0 } else { 1 })
        }
        ApprovalsCommand::Export {
            approvals_state,
            tenant,
            since_ms,
            out,
        } => {
            let result = run_approvals_export(ApprovalsExportArgs {
                approvals_state,
                tenant_id: tenant,
                since_ms,
            })?;
            let entries: Vec<serde_json::Value> = result
                .approvals
                .iter()
                .map(|e| {
                    serde_json::json!({
                        "approval": approval_summary_value(&e.approval),
                        "audit_events": e.audit_events.iter().map(audit_event_value).collect::<Vec<_>>(),
                    })
                })
                .collect();
            let payload = serde_json::json!({
                "tenant_id": result.tenant_id,
                "approvals": entries,
            });
            let serialized = serde_json::to_string_pretty(&payload)?;
            if let Some(path) = out {
                std::fs::write(&path, &serialized)
                    .with_context(|| format!("writing export to `{}`", path.display()))?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "wrote_to": path,
                        "approval_count": result.approvals.len(),
                    }))?
                );
            } else {
                println!("{serialized}");
            }
            Ok(0)
        }
    }
}


