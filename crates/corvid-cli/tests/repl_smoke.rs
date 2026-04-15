use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn repl_evaluates_a_turn_and_exits_on_eof() {
    let exe = env!("CARGO_BIN_EXE_corvid");
    let mut child = Command::new(exe)
        .arg("repl")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn corvid repl");

    {
        let stdin = child.stdin.as_mut().expect("child stdin");
        stdin.write_all(b"1 + 1\n").expect("write stdin");
    }

    let output = child.wait_with_output().expect("wait for repl");
    assert!(
        output.status.success(),
        "repl failed with status {:?}",
        output.status
    );

    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    let stdout = stdout.replace("\r\n", "\n");
    assert!(stdout.contains("2"), "unexpected repl stdout: {stdout}");
}
