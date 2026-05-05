//! Miscellaneous CLI dispatch surfaces — one-off commands too
//! small for their own per-topic modules:
//!
//! - [`cmd_new`] scaffolds a fresh project skeleton.
//! - [`cmd_check`] runs the compile pipeline up to typecheck and
//!   surfaces diagnostics without producing artifacts.
//! - [`cmd_repl`] launches the interactive REPL.
//! - [`cmd_effect_diff`] compares the effect profile of two
//!   revisions of a source file.
//! - [`cmd_add_dimension`] registers a custom-dimension entry in
//!   `corvid.toml`.
//! - [`cmd_routing_report`] renders the routing health report
//!   from local trace files.

use anyhow::{Context, Result};
use corvid_driver::{
    compile_with_config, diff_snapshots, load_corvid_config_for, render_all_pretty,
    render_effect_diff, scaffold_new, snapshot_revision, vendor_std,
};
use std::path::{Path, PathBuf};

use crate::routing_report::{
    build_report, render_report as render_routing_report, RoutingReportOptions,
};

pub(crate) fn cmd_new(name: &str) -> Result<u8> {
    let root = scaffold_new(name).context("failed to scaffold project")?;
    println!("created new Corvid project at `{}`", root.display());
    match vendor_std(&root) {
        Ok(Some(src)) => println!("vendored stdlib from `{}`", src.display()),
        Ok(None) => {}
        Err(err) => eprintln!("warning: failed to vendor stdlib: {err:#}"),
    }
    println!("\nNext steps:");
    println!("  cd {name}");
    println!("  pip install corvid-runtime");
    println!("  corvid run src/main.cor");
    Ok(0)
}

pub(crate) fn cmd_check(file: &Path) -> Result<u8> {
    let source = std::fs::read_to_string(file)
        .with_context(|| format!("cannot read `{}`", file.display()))?;
    let config = load_corvid_config_for(file);
    let result = compile_with_config(&source, config.as_ref());
    if result.ok() {
        println!("ok: {} — no errors", file.display());
        Ok(0)
    } else {
        eprint!("{}", render_all_pretty(&result.diagnostics, file, &source));
        Ok(1)
    }
}


pub(crate) fn cmd_repl() -> Result<u8> {
    corvid_repl::Repl::run_stdio().context("failed to run `corvid repl`")?;
    Ok(0)
}


// ------------------------------------------------------------
// Verification suites — effect-system spec, custom dimensions,
// adversarial bypass generation
// ------------------------------------------------------------


// ------------------------------------------------------------
// Effect-diff tool
// ------------------------------------------------------------

pub(crate) fn cmd_effect_diff(before: &str, after: &str) -> Result<u8> {
    let before_path = PathBuf::from(before);
    let after_path = PathBuf::from(after);
    println!(
        "corvid effect-diff {} -> {}\n",
        before_path.display(),
        after_path.display(),
    );
    let before_snap = snapshot_revision(&before_path)
        .with_context(|| format!("failed to snapshot `{}`", before_path.display()))?;
    let after_snap = snapshot_revision(&after_path)
        .with_context(|| format!("failed to snapshot `{}`", after_path.display()))?;
    let diff = diff_snapshots(&before_snap, &after_snap);
    print!("{}", render_effect_diff(&diff));
    // Exit 1 when the diff is non-empty so CI can gate on
    // "unexpected effect-shape drift" if the user wants it.
    let any_change = !diff.added.is_empty() || !diff.removed.is_empty() || !diff.changed.is_empty();
    Ok(if any_change { 1 } else { 0 })
}

// ------------------------------------------------------------
// Dimension registry client
// ------------------------------------------------------------

pub(crate) fn cmd_add_dimension(spec: &str, registry: Option<&str>) -> Result<u8> {
    let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    println!("corvid add-dimension {spec}\n");
    let outcome = corvid_driver::install_dimension_with_registry(spec, &project_dir, registry)?;
    match outcome {
        corvid_driver::AddDimensionOutcome::Added { name, target } => {
            println!("installed `{name}` into {}", target.display());
            println!("run `corvid test dimensions` to re-verify every dimension.");
            Ok(0)
        }
        corvid_driver::AddDimensionOutcome::Rejected { reason } => {
            eprintln!("rejected: {reason}");
            Ok(1)
        }
    }
}


pub(crate) fn cmd_routing_report(
    trace_dir: Option<&Path>,
    since: Option<&str>,
    since_commit: Option<&str>,
    json: bool,
) -> Result<u8> {
    let trace_dir = trace_dir.unwrap_or_else(|| Path::new("target/trace"));
    let report = build_report(RoutingReportOptions {
        trace_dir,
        since,
        since_commit,
    })?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", render_routing_report(&report));
    }
    Ok(if report.healthy { 0 } else { 1 })
}
