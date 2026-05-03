use std::process::Command;

#[test]
fn package_help_states_hosted_registry_boundary() {
    let output = Command::new(env!("CARGO_BIN_EXE_corvid"))
        .args(["package", "--help"])
        .output()
        .expect("run corvid package --help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("format-and-tooling only")
            && stdout.contains("no Corvid-hosted registry service runs yet"),
        "{stdout}"
    );
}

#[test]
fn add_help_requires_explicit_registry_source() {
    let output = Command::new(env!("CARGO_BIN_EXE_corvid"))
        .args(["add", "--help"])
        .output()
        .expect("run corvid add --help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("No Corvid-hosted package registry runs yet")
            && stdout.contains("CORVID_PACKAGE_REGISTRY"),
        "{stdout}"
    );
}
