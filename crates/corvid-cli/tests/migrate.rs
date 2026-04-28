use std::path::PathBuf;
use std::process::Command;

fn corvid_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_corvid"))
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
        assert!(stdout.contains("no state was changed"), "{stdout}");
    }
}
