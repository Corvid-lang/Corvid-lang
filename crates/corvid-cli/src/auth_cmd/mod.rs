//! `corvid auth` CLI surface — slice 39L, decomposed in
//! Phase 20j-S1.
//!
//! Wires the Phase 39 auth runtime into the top-level `corvid`
//! CLI so an operator can manage sessions / API keys / OAuth
//! tokens from the shell rather than only from Rust callers. The
//! runtime functions (`SessionAuthRuntime::create_api_key`, etc.)
//! are unchanged; this slice contributes only the clap surface +
//! JSON-rendering of the runtime's typed records.
//!
//! `--auth-state` and `--approvals-state` default to
//! `target/auth.db` and `target/approvals.db` respectively. Both
//! file paths are SQLite databases initialised on first open;
//! `corvid auth migrate` is the explicit "open both, init both,
//! report success" operation an operator runs once at deploy.
//!
//! After Phase 20j-S1 the module holds two responsibilities only:
//!
//! - The deploy-time migrator (`run_auth_migrate`) — small enough
//!   to live in the module root.
//! - The API-key lifecycle re-exported from [`keys`].
//!
//! The `corvid approvals *` surface is the sibling
//! [`crate::approvals_cmd`] module; auth and approval lanes
//! evolve independently from this slice forward.

pub mod keys;
#[allow(unused_imports)]
pub use keys::*;

use anyhow::{anyhow, Context, Result};
use corvid_runtime::approval_queue::ApprovalQueueRuntime;
use corvid_runtime::auth::SessionAuthRuntime;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct AuthMigrateArgs {
    pub auth_state: PathBuf,
    pub approvals_state: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuthMigrateOutput {
    pub auth_state: PathBuf,
    pub approvals_state: PathBuf,
    pub auth_initialised: bool,
    pub approvals_initialised: bool,
}

/// Open both stores at the supplied paths; both runtimes' `open`
/// constructors invoke `init()` to create tables idempotently. The
/// command is safe to run any number of times.
pub fn run_auth_migrate(args: AuthMigrateArgs) -> Result<AuthMigrateOutput> {
    if let Some(parent) = args.auth_state.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("creating auth state parent `{}`", parent.display())
            })?;
        }
    }
    if let Some(parent) = args.approvals_state.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("creating approvals state parent `{}`", parent.display())
            })?;
        }
    }
    let _auth = SessionAuthRuntime::open(&args.auth_state)
        .map_err(|e| anyhow!("auth runtime init failed: {e}"))?;
    let _approvals = ApprovalQueueRuntime::open(&args.approvals_state)
        .map_err(|e| anyhow!("approvals runtime init failed: {e}"))?;
    Ok(AuthMigrateOutput {
        auth_state: args.auth_state,
        approvals_state: args.approvals_state,
        auth_initialised: true,
        approvals_initialised: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn temp_paths() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempdir().unwrap();
        let auth = dir.path().join("auth.db");
        let approvals = dir.path().join("approvals.db");
        (dir, auth, approvals)
    }

    /// Slice 39L: `corvid auth migrate` opens both stores
    /// idempotently. Re-running is a no-op.
    #[test]
    fn migrate_creates_state_files_idempotently() {
        let (_dir, auth, approvals) = temp_paths();
        let out = run_auth_migrate(AuthMigrateArgs {
            auth_state: auth.clone(),
            approvals_state: approvals.clone(),
        })
        .expect("migrate");
        assert!(out.auth_initialised);
        assert!(out.approvals_initialised);
        assert!(auth.exists());
        assert!(approvals.exists());
        // Re-run is a no-op.
        let out2 = run_auth_migrate(AuthMigrateArgs {
            auth_state: auth,
            approvals_state: approvals,
        })
        .expect("re-migrate");
        assert!(out2.auth_initialised);
    }
}
