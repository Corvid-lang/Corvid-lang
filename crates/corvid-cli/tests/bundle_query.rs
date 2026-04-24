mod bundle_support;

use bundle_support::{create_linked_fixture, run_corvid};

#[test]
fn bundle_query_isolates_requested_delta_against_predecessor() {
    let fixture = create_linked_fixture();

    let output = run_corvid(
        &[
            "bundle",
            "query",
            fixture.head.root.to_str().expect("utf8 root"),
            "--delta",
            "agent.replayable_gained:classify",
            "--json",
        ],
        &fixture.head.root,
    );
    assert!(
        output.status.success(),
        "bundle query failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("agent.replayable_gained:classify"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("\"isolated_attestation_diff\""),
        "stdout was: {stdout}"
    );
}

#[test]
fn bundle_query_rejects_unknown_delta_class() {
    let fixture = create_linked_fixture();

    let output = run_corvid(
        &[
            "bundle",
            "query",
            fixture.head.root.to_str().expect("utf8 root"),
            "--delta",
            "agent.approval.label_added:classify:EchoString",
            "--json",
        ],
        &fixture.head.root,
    );
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("BundleCounterfactualUnsupported"),
        "stderr was: {stderr}"
    );
}
