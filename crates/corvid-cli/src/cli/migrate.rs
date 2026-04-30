//! `corvid migrate` clap argument tree — slice 27 / migrations
//! surface, decomposed in Phase 20j-A1.
//!
//! Owns the [`MigrateCommand`] subcommand enum that the
//! `corvid migrate status|up|down` dispatch arms in main.rs
//! consume.

use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum MigrateCommand {
    /// Report applied, pending, and drifted migrations.
    Status {
        /// Directory containing ordered `.sql` migration files.
        #[arg(long, value_name = "DIR", default_value = "migrations")]
        dir: PathBuf,
        /// State file used to record applied migration checksums.
        #[arg(
            long,
            value_name = "PATH",
            default_value = "target/corvid-migrations.json"
        )]
        state: PathBuf,
        /// SQLite database file migrations are checked against.
        #[arg(long, value_name = "PATH", default_value = "target/corvid.sqlite")]
        database: PathBuf,
        /// Show what would be checked without writing state.
        #[arg(long)]
        dry_run: bool,
    },
    /// Apply pending migrations in order.
    Up {
        /// Directory containing ordered `.sql` migration files.
        #[arg(long, value_name = "DIR", default_value = "migrations")]
        dir: PathBuf,
        /// State file used to record applied migration checksums.
        #[arg(
            long,
            value_name = "PATH",
            default_value = "target/corvid-migrations.json"
        )]
        state: PathBuf,
        /// SQLite database file migrations are executed against.
        #[arg(long, value_name = "PATH", default_value = "target/corvid.sqlite")]
        database: PathBuf,
        /// Report pending migrations without applying them.
        #[arg(long)]
        dry_run: bool,
    },
    /// Roll back the latest migration when a down migration exists.
    Down {
        /// Directory containing ordered `.sql` migration files.
        #[arg(long, value_name = "DIR", default_value = "migrations")]
        dir: PathBuf,
        /// Directory containing reviewed rollback SQL files named `<migration>.down.sql`.
        #[arg(long, value_name = "DIR", default_value = "migrations/down")]
        down_dir: PathBuf,
        /// State file used to record applied migration checksums.
        #[arg(
            long,
            value_name = "PATH",
            default_value = "target/corvid-migrations.json"
        )]
        state: PathBuf,
        /// SQLite database file associated with migration state.
        #[arg(long, value_name = "PATH", default_value = "target/corvid.sqlite")]
        database: PathBuf,
        /// Report the rollback candidate without mutating state.
        #[arg(long)]
        dry_run: bool,
    },
}
