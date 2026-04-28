use std::path::PathBuf;
use std::process::Command;

fn corvid_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_corvid"))
}

#[test]
fn migrate_status_lists_ordered_sql_files_with_checksums() {
    let dir = tempfile::tempdir().expect("tempdir");
    let migrations = dir.path().join("migrations");
    std::fs::create_dir_all(&migrations).expect("migrations dir");
    std::fs::write(
        migrations.join("0002_add_tasks.sql"),
        "create table tasks(id text);\n",
    )
    .expect("write migration 2");
    std::fs::write(
        migrations.join("0001_init.sql"),
        "create table users(id text);\n",
    )
    .expect("write migration 1");
    std::fs::write(migrations.join("README.md"), "ignored\n").expect("write ignored");

    let out = Command::new(corvid_bin())
        .args([
            "migrate",
            "status",
            "--dir",
            migrations.to_str().unwrap(),
            "--dry-run",
        ])
        .output()
        .expect("run migrate status");
    assert!(
        out.status.success(),
        "status failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("migrations_found: 2"), "{stdout}");
    let first = stdout.find("0001_init.sql").expect("0001 listed");
    let second = stdout.find("0002_add_tasks.sql").expect("0002 listed");
    assert!(first < second, "{stdout}");
    assert!(stdout.contains("sha256:"), "{stdout}");
    assert!(!stdout.contains("README.md"), "{stdout}");
}

#[test]
fn migrate_up_records_applied_state_and_status_reads_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let migrations = dir.path().join("migrations");
    let state = dir.path().join("target").join("corvid-migrations.json");
    let database = dir.path().join("target").join("app.sqlite");
    std::fs::create_dir_all(&migrations).expect("migrations dir");
    std::fs::write(
        migrations.join("0001_init.sql"),
        "create table users(id text);\n",
    )
    .expect("write migration");

    let up = Command::new(corvid_bin())
        .args([
            "migrate",
            "up",
            "--dir",
            migrations.to_str().unwrap(),
            "--state",
            state.to_str().unwrap(),
            "--database",
            database.to_str().unwrap(),
        ])
        .output()
        .expect("run migrate up");
    assert!(
        up.status.success(),
        "up failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&up.stdout),
        String::from_utf8_lossy(&up.stderr)
    );
    let up_stdout = String::from_utf8_lossy(&up.stdout);
    assert!(up_stdout.contains("state_updated: true"), "{up_stdout}");
    assert!(state.exists(), "state file should be written");

    let status = Command::new(corvid_bin())
        .args([
            "migrate",
            "status",
            "--dir",
            migrations.to_str().unwrap(),
            "--state",
            state.to_str().unwrap(),
            "--database",
            database.to_str().unwrap(),
        ])
        .output()
        .expect("run migrate status");
    assert!(
        status.status.success(),
        "status failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&status.stdout),
        String::from_utf8_lossy(&status.stderr)
    );
    let status_stdout = String::from_utf8_lossy(&status.stdout);
    assert!(status_stdout.contains("status:applied"), "{status_stdout}");
}

#[test]
fn migrate_up_executes_sql_against_sqlite_database() {
    let dir = tempfile::tempdir().expect("tempdir");
    let migrations = dir.path().join("migrations");
    let state = dir.path().join("target").join("corvid-migrations.json");
    let database = dir.path().join("target").join("app.sqlite");
    std::fs::create_dir_all(&migrations).expect("migrations dir");
    std::fs::write(
        migrations.join("0001_init.sql"),
        "create table users(id text primary key, email text not null);\n",
    )
    .expect("write migration");

    let up = Command::new(corvid_bin())
        .args([
            "migrate",
            "up",
            "--dir",
            migrations.to_str().unwrap(),
            "--state",
            state.to_str().unwrap(),
            "--database",
            database.to_str().unwrap(),
        ])
        .output()
        .expect("run migrate up");
    assert!(
        up.status.success(),
        "up failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&up.stdout),
        String::from_utf8_lossy(&up.stderr)
    );

    let conn = rusqlite::Connection::open(&database).expect("open migrated database");
    let table_count: i64 = conn
        .query_row(
            "select count(*) from sqlite_master where type = 'table' and name = 'users'",
            [],
            |row| row.get(0),
        )
        .expect("query sqlite schema");
    assert_eq!(table_count, 1, "migration SQL should create users table");
    assert!(
        state.exists(),
        "state file should be written after SQL succeeds"
    );
}

#[test]
fn migrate_up_does_not_record_state_when_sql_fails() {
    let dir = tempfile::tempdir().expect("tempdir");
    let migrations = dir.path().join("migrations");
    let state = dir.path().join("target").join("corvid-migrations.json");
    let database = dir.path().join("target").join("app.sqlite");
    std::fs::create_dir_all(&migrations).expect("migrations dir");
    std::fs::write(migrations.join("0001_broken.sql"), "create table broken(\n")
        .expect("write migration");

    let up = Command::new(corvid_bin())
        .args([
            "migrate",
            "up",
            "--dir",
            migrations.to_str().unwrap(),
            "--state",
            state.to_str().unwrap(),
            "--database",
            database.to_str().unwrap(),
        ])
        .output()
        .expect("run migrate up");
    assert!(!up.status.success(), "broken migration should fail");
    assert!(
        !state.exists(),
        "state file must not be written when SQL execution fails"
    );
}

#[test]
fn migrate_status_reports_changed_missing_duplicate_and_out_of_order_drift() {
    let dir = tempfile::tempdir().expect("tempdir");
    let migrations = dir.path().join("migrations");
    let state = dir.path().join("target").join("corvid-migrations.json");
    std::fs::create_dir_all(&migrations).expect("migrations dir");
    std::fs::create_dir_all(state.parent().unwrap()).expect("state dir");
    std::fs::write(migrations.join("0001_init.sql"), "changed\n").expect("write 0001");
    std::fs::write(migrations.join("0001_duplicate.sql"), "duplicate\n").expect("write duplicate");
    std::fs::write(migrations.join("0002_second.sql"), "second\n").expect("write 0002");
    std::fs::write(
        &state,
        r#"{
  "migrations": [
    {"name":"0002_second.sql","sha256":"0000000000000000000000000000000000000000000000000000000000000000","applied_at":2},
    {"name":"0001_init.sql","sha256":"1111111111111111111111111111111111111111111111111111111111111111","applied_at":1},
    {"name":"0000_missing.sql","sha256":"2222222222222222222222222222222222222222222222222222222222222222","applied_at":0}
  ]
}"#,
    )
    .expect("write state");

    let out = Command::new(corvid_bin())
        .args([
            "migrate",
            "status",
            "--dir",
            migrations.to_str().unwrap(),
            "--state",
            state.to_str().unwrap(),
        ])
        .output()
        .expect("run migrate status");
    assert!(!out.status.success(), "drift should fail");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("drift: changed"), "{stdout}");
    assert!(stdout.contains("drift: missing"), "{stdout}");
    assert!(stdout.contains("drift: duplicate"), "{stdout}");
    assert!(stdout.contains("drift: out_of_order"), "{stdout}");
    assert!(stdout.contains("drift_found:"), "{stdout}");
}

#[test]
fn migrate_dry_run_reports_counts_without_state_mutation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let migrations = dir.path().join("migrations");
    let state = dir.path().join("target").join("corvid-migrations.json");
    std::fs::create_dir_all(&migrations).expect("migrations dir");
    std::fs::write(
        migrations.join("0001_init.sql"),
        "create table users(id text);\n",
    )
    .expect("write migration");

    let out = Command::new(corvid_bin())
        .args([
            "migrate",
            "up",
            "--dir",
            migrations.to_str().unwrap(),
            "--state",
            state.to_str().unwrap(),
            "--dry-run",
        ])
        .output()
        .expect("run migrate up dry run");
    assert!(
        out.status.success(),
        "dry-run failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("applied_count: 0"), "{stdout}");
    assert!(stdout.contains("pending_count: 1"), "{stdout}");
    assert!(stdout.contains("drift_count: 0"), "{stdout}");
    assert!(stdout.contains("state_updated: false"), "{stdout}");
    assert!(stdout.contains("mutation_intent: none"), "{stdout}");
    assert!(!state.exists(), "dry-run must not write state");
}

#[test]
fn state_app_migration_schema_reports_pending() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let migrations = repo
        .join("examples")
        .join("backend")
        .join("state_app")
        .join("migrations");
    let state_dir = tempfile::tempdir().expect("tempdir");
    let state = state_dir.path().join("corvid-migrations.json");

    let out = Command::new(corvid_bin())
        .args([
            "migrate",
            "status",
            "--dir",
            migrations.to_str().unwrap(),
            "--state",
            state.to_str().unwrap(),
            "--dry-run",
        ])
        .output()
        .expect("run state app migration status");
    assert!(
        out.status.success(),
        "status failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("0001_core_state.sql"), "{stdout}");
    assert!(stdout.contains("pending_count: 1"), "{stdout}");
}

#[test]
fn migrate_help_lists_status_up_down() {
    let out = Command::new(corvid_bin())
        .args(["migrate", "--help"])
        .output()
        .expect("run corvid migrate help");
    assert!(
        out.status.success(),
        "help failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("status"), "{stdout}");
    assert!(stdout.contains("up"), "{stdout}");
    assert!(stdout.contains("down"), "{stdout}");
}

#[test]
fn migrate_subcommands_accept_dry_run_shape() {
    let dir = tempfile::tempdir().expect("tempdir");
    let migrations = dir.path().join("migrations");
    let state = dir.path().join("target").join("corvid-migrations.json");

    for action in ["status", "up", "down"] {
        let out = Command::new(corvid_bin())
            .args([
                "migrate",
                action,
                "--dir",
                migrations.to_str().unwrap(),
                "--state",
                state.to_str().unwrap(),
                "--dry-run",
            ])
            .output()
            .expect("run corvid migrate");
        assert!(
            out.status.success(),
            "{action} failed:\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains(&format!("corvid migrate {action}")),
            "{stdout}"
        );
        assert!(stdout.contains("dry_run: true"), "{stdout}");
        assert!(stdout.contains("state_updated: false"), "{stdout}");
    }
}
