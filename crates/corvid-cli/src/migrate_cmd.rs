//! `corvid migrate` CLI dispatch — slice 27 / 38 migrations
//! surface, decomposed in Phase 20j-A1.
//!
//! Two entry points:
//!
//! - [`cmd_migrate`] handles the `up` / `inspect` subcommands —
//!   scans the migrations directory, classifies pending vs
//!   applied vs drift, and (for `up`) applies pending SQL
//!   transactions atomically while updating the recorded state.
//! - [`cmd_migrate_down`] handles the `down` subcommand —
//!   identifies the latest applied migration, locates its
//!   `<name>.down.sql` rollback file, and executes the
//!   rollback as a single transaction.
//!
//! All migration discovery / state I/O / SQL apply / SQL rollback
//! helpers are private to this module; main.rs's dispatch arm
//! only needs the two `cmd_migrate*` entry points.

use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub(crate) fn cmd_migrate(
    action: &str,
    dir: &Path,
    state: &Path,
    database: &Path,
    dry_run: bool,
) -> Result<u8> {
    let migrations = scan_migration_files(dir)?;
    let mut migration_state = load_migration_state(state)?;
    let drift = detect_migration_drift(&migrations, &migration_state);
    println!("corvid migrate {action}");
    println!("migrations: {}", dir.display());
    println!("state: {}", state.display());
    println!("database: {}", database.display());
    println!("dry_run: {dry_run}");
    let applied_count = migrations
        .iter()
        .filter(|migration| {
            migration_state
                .migrations
                .iter()
                .any(|applied| applied.name == migration.name && applied.sha256 == migration.sha256)
        })
        .count();
    let pending_count = migrations.len().saturating_sub(applied_count);
    println!("applied_count: {applied_count}");
    println!("pending_count: {pending_count}");
    println!("drift_count: {}", drift.len());
    if migrations.is_empty() {
        println!("migrations_found: 0");
    } else {
        println!("migrations_found: {}", migrations.len());
        for migration in &migrations {
            let applied = migration_state.migrations.iter().any(|applied| {
                applied.name == migration.name && applied.sha256 == migration.sha256
            });
            println!(
                "migration: {} sha256:{} status:{}",
                migration.name,
                migration.sha256,
                if applied { "applied" } else { "pending" }
            );
        }
    }
    for item in &drift {
        println!("drift: {} {}", item.kind, item.message);
    }
    if !drift.is_empty() {
        println!("drift_found: {}", drift.len());
        println!("state_updated: false");
        return Ok(1);
    }
    if action == "up" && !dry_run {
        apply_pending_sql_migrations(database, &migrations, &mut migration_state)?;
        save_migration_state(state, &migration_state)?;
        println!("state_updated: true");
    } else {
        println!("state_updated: false");
    }
    println!(
        "mutation_intent: {}",
        if action == "up" && !dry_run && pending_count > 0 {
            "apply_pending"
        } else if action == "down" && !dry_run {
            "rollback_latest"
        } else {
            "none"
        }
    );
    Ok(0)
}

pub(crate) fn cmd_migrate_down(
    dir: &Path,
    down_dir: &Path,
    state: &Path,
    database: &Path,
    dry_run: bool,
) -> Result<u8> {
    let migrations = scan_migration_files(dir)?;
    let mut migration_state = load_migration_state(state)?;
    let drift = detect_migration_drift(&migrations, &migration_state);
    println!("corvid migrate down");
    println!("migrations: {}", dir.display());
    println!("down_migrations: {}", down_dir.display());
    println!("state: {}", state.display());
    println!("database: {}", database.display());
    println!("dry_run: {dry_run}");
    println!("applied_count: {}", migration_state.migrations.len());
    println!("drift_count: {}", drift.len());
    for item in &drift {
        println!("drift: {} {}", item.kind, item.message);
    }
    if !drift.is_empty() {
        println!("drift_found: {}", drift.len());
        println!("state_updated: false");
        return Ok(1);
    }
    let Some(latest) = migration_state.migrations.last().cloned() else {
        println!("rollback: none");
        println!("state_updated: false");
        println!("mutation_intent: none");
        return Ok(0);
    };
    let rollback = rollback_migration_path(down_dir, &latest.name);
    println!("rollback: {}", latest.name);
    println!("rollback_sql: {}", rollback.display());
    if !rollback.exists() {
        println!("state_updated: false");
        return Err(anyhow::anyhow!(
            "missing rollback SQL `{}` for `{}`",
            rollback.display(),
            latest.name
        ));
    }
    if dry_run {
        println!("state_updated: false");
        println!("mutation_intent: rollback_latest");
        return Ok(0);
    }
    execute_rollback_sql(database, &rollback, &latest.name)?;
    migration_state.migrations.pop();
    save_migration_state(state, &migration_state)?;
    println!("state_updated: true");
    println!("mutation_intent: rollback_latest");
    Ok(0)
}

struct MigrationFile {
    name: String,
    sha256: String,
    path: PathBuf,
}

struct MigrationDrift {
    kind: &'static str,
    message: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct MigrationState {
    migrations: Vec<AppliedMigration>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AppliedMigration {
    name: String,
    sha256: String,
    applied_at: u64,
}

fn scan_migration_files(dir: &Path) -> Result<Vec<MigrationFile>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("cannot read migrations directory `{}`", dir.display()))?
    {
        let entry =
            entry.with_context(|| format!("cannot read entry under `{}`", dir.display()))?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("sql") {
            continue;
        }
        let bytes = std::fs::read(&path)
            .with_context(|| format!("cannot read migration `{}`", path.display()))?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("<invalid>")
            .to_string();
        let sha256 = hex::encode(Sha256::digest(&bytes));
        files.push(MigrationFile { name, sha256, path });
    }
    files.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(files)
}

fn detect_migration_drift(
    migrations: &[MigrationFile],
    state: &MigrationState,
) -> Vec<MigrationDrift> {
    let mut drift = Vec::new();
    let mut seen_versions = std::collections::HashMap::<String, String>::new();
    for migration in migrations {
        let version = migration_version(&migration.name);
        if let Some(previous) = seen_versions.insert(version.clone(), migration.name.clone()) {
            drift.push(MigrationDrift {
                kind: "duplicate",
                message: format!(
                    "version `{version}` appears in `{previous}` and `{}`",
                    migration.name
                ),
            });
        }
    }

    for applied in &state.migrations {
        match migrations
            .iter()
            .find(|migration| migration.name == applied.name)
        {
            Some(current) if current.sha256 != applied.sha256 => drift.push(MigrationDrift {
                kind: "changed",
                message: format!(
                    "`{}` expected sha256:{}, actual sha256:{}",
                    applied.name, applied.sha256, current.sha256
                ),
            }),
            Some(_) => {}
            None => drift.push(MigrationDrift {
                kind: "missing",
                message: format!("applied migration `{}` is missing from disk", applied.name),
            }),
        }
    }

    let file_order = migrations
        .iter()
        .map(|migration| migration.name.as_str())
        .collect::<Vec<_>>();
    let mut last_index = None;
    for applied in &state.migrations {
        if let Some(index) = file_order.iter().position(|name| *name == applied.name) {
            if last_index.is_some_and(|last| index < last) {
                drift.push(MigrationDrift {
                    kind: "out_of_order",
                    message: format!(
                        "applied migration `{}` is earlier than a previously applied migration",
                        applied.name
                    ),
                });
            }
            last_index = Some(index);
        }
    }

    drift
}

fn migration_version(name: &str) -> String {
    name.split_once('_')
        .map(|(version, _)| version)
        .or_else(|| name.split_once('.').map(|(version, _)| version))
        .unwrap_or(name)
        .to_string()
}

fn load_migration_state(path: &Path) -> Result<MigrationState> {
    if !path.exists() {
        return Ok(MigrationState::default());
    }
    let bytes = std::fs::read(path)
        .with_context(|| format!("cannot read migration state `{}`", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("cannot parse migration state `{}`", path.display()))
}

fn save_migration_state(path: &Path, state: &MigrationState) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create migration state dir `{}`", parent.display()))?;
    }
    let json = serde_json::to_vec_pretty(state).context("cannot serialize migration state")?;
    std::fs::write(path, json)
        .with_context(|| format!("cannot write migration state `{}`", path.display()))
}

fn now_unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}


fn apply_pending_sql_migrations(
    database: &Path,
    migrations: &[MigrationFile],
    state: &mut MigrationState,
) -> Result<()> {
    if let Some(parent) = database
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create database dir `{}`", parent.display()))?;
    }
    let mut conn = Connection::open(database)
        .with_context(|| format!("cannot open migration database `{}`", database.display()))?;
    for migration in migrations {
        let applied = state
            .migrations
            .iter()
            .any(|applied| applied.name == migration.name && applied.sha256 == migration.sha256);
        if applied {
            continue;
        }
        let sql = std::fs::read_to_string(&migration.path)
            .with_context(|| format!("cannot read migration SQL `{}`", migration.path.display()))?;
        let tx = conn
            .transaction()
            .with_context(|| format!("cannot start transaction for `{}`", migration.name))?;
        tx.execute_batch(&sql)
            .with_context(|| format!("cannot execute migration `{}`", migration.name))?;
        tx.commit()
            .with_context(|| format!("cannot commit migration `{}`", migration.name))?;
        state.migrations.push(AppliedMigration {
            name: migration.name.clone(),
            sha256: migration.sha256.clone(),
            applied_at: now_unix_seconds(),
        });
    }
    Ok(())
}

fn rollback_migration_path(down_dir: &Path, applied_name: &str) -> PathBuf {
    down_dir.join(format!("{applied_name}.down.sql"))
}

fn execute_rollback_sql(database: &Path, rollback: &Path, applied_name: &str) -> Result<()> {
    let sql = std::fs::read_to_string(rollback)
        .with_context(|| format!("cannot read rollback SQL `{}`", rollback.display()))?;
    let mut conn = Connection::open(database)
        .with_context(|| format!("cannot open migration database `{}`", database.display()))?;
    let tx = conn
        .transaction()
        .with_context(|| format!("cannot start rollback transaction for `{applied_name}`"))?;
    tx.execute_batch(&sql)
        .with_context(|| format!("cannot execute rollback for `{applied_name}`"))?;
    tx.commit()
        .with_context(|| format!("cannot commit rollback for `{applied_name}`"))?;
    Ok(())
}
