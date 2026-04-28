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
    std::fs::write(migrations.join("0002_add_tasks.sql"), "create table tasks(id text);\n")
        .expect("write migration 2");
    std::fs::write(migrations.join("0001_init.sql"), "create table users(id text);\n")
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
    std::fs::create_dir_all(&migrations).expect("migrations dir");
    std::fs::write(migrations.join("0001_init.sql"), "create table users(id text);\n")
        .expect("write migration");

    let up = Command::new(corvid_bin())
        .args([
            "migrate",
            "up",
            "--dir",
            migrations.to_str().unwrap(),
            "--state",
            state.to_str().unwrap(),
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
        assert!(stdout.contains(&format!("corvid migrate {action}")), "{stdout}");
        assert!(stdout.contains("dry_run: true"), "{stdout}");
        assert!(stdout.contains("state_updated: false"), "{stdout}");
    }
}
