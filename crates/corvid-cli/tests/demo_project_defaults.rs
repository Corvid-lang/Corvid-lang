use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn corvid_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_corvid"))
}

#[test]
fn build_and_run_default_to_project_main_source() {
    let app = repo_root().join("examples").join("refund_bot");

    let build = Command::new(corvid_bin())
        .arg("build")
        .current_dir(&app)
        .output()
        .expect("run corvid build");
    assert!(
        build.status.success(),
        "build failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
    let build_stdout = String::from_utf8_lossy(&build.stdout);
    assert!(build_stdout.contains("src\\main.cor") || build_stdout.contains("src/main.cor"));
    assert!(
        app.join("target").join("py").join("main.py").exists(),
        "default build should emit target/py/main.py"
    );

    let run = Command::new(corvid_bin())
        .arg("run")
        .current_dir(&app)
        .output()
        .expect("run corvid run");
    assert!(
        run.status.success(),
        "run failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let run_stdout = String::from_utf8_lossy(&run.stdout);
    assert!(run_stdout.contains("refund_bot"), "{run_stdout}");
    assert!(run_stdout.contains("approval-gated refund"), "{run_stdout}");
}
