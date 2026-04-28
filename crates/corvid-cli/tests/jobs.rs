use std::path::PathBuf;
use std::process::Command;

fn corvid_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_corvid"))
}

#[test]
fn jobs_enqueue_and_run_one_persist_state() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("jobs.sqlite");

    let enqueue = Command::new(corvid_bin())
        .args([
            "jobs",
            "enqueue",
            "--state",
            state.to_str().unwrap(),
            "--task",
            "daily_brief",
            "--payload",
            "{\"user\":\"u1\"}",
            "--max-retries",
            "2",
            "--budget-usd",
            "0.50",
            "--effect-summary",
            "llm+db",
            "--replay-key",
            "replay:daily:u1",
        ])
        .output()
        .expect("run jobs enqueue");
    assert!(
        enqueue.status.success(),
        "enqueue failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&enqueue.stdout),
        String::from_utf8_lossy(&enqueue.stderr)
    );
    let stdout = String::from_utf8_lossy(&enqueue.stdout);
    assert!(stdout.contains("corvid jobs enqueue"), "{stdout}");
    assert!(stdout.contains("task: daily_brief"), "{stdout}");
    assert!(stdout.contains("status: pending"), "{stdout}");
    assert!(stdout.contains("effect_summary: llm+db"), "{stdout}");

    let run = Command::new(corvid_bin())
        .args(["jobs", "run-one", "--state", state.to_str().unwrap()])
        .output()
        .expect("run jobs run-one");
    assert!(
        run.status.success(),
        "run-one failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(stdout.contains("corvid jobs run-one"), "{stdout}");
    assert!(stdout.contains("task: daily_brief"), "{stdout}");
    assert!(stdout.contains("status: succeeded"), "{stdout}");
    assert!(stdout.contains("attempts: 1"), "{stdout}");
}
