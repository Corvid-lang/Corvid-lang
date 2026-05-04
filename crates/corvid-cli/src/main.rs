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
mod approvals_cmd;
mod approver_cmd;
mod audit_cmd;
mod auth_cmd;
mod bench_cmd;
mod bind_cmd;
mod build_cmd;
mod bundle_cmd;
mod capsule_cmd;
mod claim_cmd;
mod cli;
mod commands;
mod connectors_cmd;
mod contract_cmd;
mod cost_frontier;
mod deploy_cmd;
mod dispatch;
mod doctor_cmd;
mod eval_cmd;
mod format;
mod lineage_cmd;
mod migrate_cmd;
mod observe_cmd;
mod observe_helpers_cmd;
mod package_cmd;
mod project_source;
mod receipt_cache;
mod receipt_cmd;
mod release_cmd;
mod replay;
mod routing_report;
mod run_cmd;
mod test_from_traces;
mod tour;
mod trace_cmd;
mod trace_dag;
mod trace_diff;
mod upgrade_cmd;
mod verify_cmd;

use clap::Parser;
use std::process::ExitCode;

use cli::root::Cli;

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
    match dispatch::run(cli) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(2)
        }
    }
}
