mod bundle_support;

use std::fs;

use bundle_support::{create_lineage_chain_fixture, run_corvid};

#[test]
fn bundle_lineage_walks_predecessor_chain_and_verifies_signatures() {
    let fixture = create_lineage_chain_fixture();

    let output = run_corvid(
        &[
            "bundle",
            "lineage",
            fixture.head.root.to_str().expect("utf8 root"),
            "--json",
        ],
        &fixture.head.root,
    );
    assert!(
        output.status.success(),
        "bundle lineage failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("lineage-head"), "stdout was: {stdout}");
    assert!(stdout.contains("lineage-mid"), "stdout was: {stdout}");
    assert!(stdout.contains("lineage-base"), "stdout was: {stdout}");
    assert!(stdout.contains("\"signature_verified\": true"), "stdout was: {stdout}");
}

#[test]
fn bundle_lineage_rejects_tampered_predecessor_signature() {
    let fixture = create_lineage_chain_fixture();
    let envelope = fixture.base.root.join("keys").join("receipt.envelope.json");
    fs::write(
        &envelope,
        br#"{"payloadType":"application/vnd.corvid-receipt+json","payload":"Zm9v","signatures":[]}"#,
    )
    .expect("tamper base envelope");

    let output = run_corvid(
        &[
            "bundle",
            "lineage",
            fixture.head.root.to_str().expect("utf8 root"),
            "--json",
        ],
        &fixture.head.root,
    );
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("BundleSignatureVerifyFailed"),
        "stderr was: {stderr}"
    );
}
