use std::path::PathBuf;
use std::process::Command;

#[test]
fn routing_report_matches_golden_fixture() {
    let exe = env!("CARGO_BIN_EXE_corvid");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let trace_dir = root.join("tests/fixtures/trace-report");
    let expected = std::fs::read_to_string(trace_dir.join("expected.txt")).expect("golden output");

    let output = Command::new(exe)
        .args([
            "routing-report",
            "--trace-dir",
            trace_dir.to_str().expect("utf8 path"),
        ])
        .current_dir(&root)
        .output()
        .expect("routing-report runs");

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(String::from_utf8(output.stdout).expect("stdout"), expected);
}
