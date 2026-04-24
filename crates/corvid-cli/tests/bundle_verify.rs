mod bundle_support;

use std::fs;

use bundle_support::{create_fixture, run_corvid};

#[test]
fn bundle_verify_accepts_happy_path_and_rejects_hash_or_signature_tampering() {
    let fixture = create_fixture();

    let ok = run_corvid(
        &["bundle", "verify", fixture.root.to_str().expect("utf8 root")],
        &fixture.root,
    );
    assert!(
        ok.status.success(),
        "bundle verify failed: stdout={} stderr={}",
        String::from_utf8_lossy(&ok.stdout),
        String::from_utf8_lossy(&ok.stderr)
    );

    fs::write(&fixture.library_path, b"tampered").expect("tamper library");
    let bad_hash = run_corvid(
        &["bundle", "verify", fixture.root.to_str().expect("utf8 root")],
        &fixture.root,
    );
    assert_eq!(bad_hash.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&bad_hash.stderr);
    assert!(stderr.contains("BundleHashMismatch"), "stderr was: {stderr}");
}

#[test]
fn bundle_verify_rejects_signature_tampering() {
    let fixture = create_fixture();
    let envelope = fixture.root.join("keys").join("receipt.envelope.json");
    fs::write(&envelope, b"{\"payloadType\":\"application/vnd.corvid-receipt+json\",\"payload\":\"Zm9v\",\"signatures\":[]}").expect("tamper envelope");
    let manifest = fixture.root.join("corvid-bundle.toml");
    let mut value: toml::Value = toml::from_str(&fs::read_to_string(&manifest).expect("read manifest"))
        .expect("parse manifest");
    value["hashes"]["receipt_envelope"] = toml::Value::String(bundle_support::sha256_file_for_tests(&envelope));
    fs::write(&manifest, toml::to_string_pretty(&value).expect("serialize manifest"))
        .expect("write manifest");
    let result = run_corvid(
        &["bundle", "verify", fixture.root.to_str().expect("utf8 root")],
        &fixture.root,
    );
    assert_eq!(result.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("BundleSignatureVerifyFailed"), "stderr was: {stderr}");
}
