mod bundle_support;

use std::fs;

use bundle_support::{create_fixture, run_corvid};

#[test]
fn bundle_verify_rebuild_accepts_happy_path_and_catches_binding_or_platform_drift() {
    let fixture = create_fixture();

    let ok = run_corvid(
        &[
            "bundle",
            "verify",
            fixture.root.to_str().expect("utf8 root"),
            "--rebuild",
        ],
        &fixture.root,
    );
    assert!(
        ok.status.success(),
        "bundle verify --rebuild failed: stdout={} stderr={}",
        String::from_utf8_lossy(&ok.stdout),
        String::from_utf8_lossy(&ok.stderr)
    );

    let rust_readme = fixture.root.join("bindings_rust").join("README.md");
    fs::write(&rust_readme, "tampered binding\n").expect("tamper rust binding");
    let mismatch = run_corvid(
        &[
            "bundle",
            "verify",
            fixture.root.to_str().expect("utf8 root"),
            "--rebuild",
        ],
        &fixture.root,
    );
    assert_eq!(mismatch.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&mismatch.stderr);
    assert!(stderr.contains("BundleHashMismatch"), "stderr was: {stderr}");
}

#[test]
fn bundle_verify_rebuild_rejects_target_mismatch() {
    let fixture = create_fixture();
    let manifest = fixture.manifest_path.clone();
    let original = fs::read_to_string(&manifest).expect("read manifest");
    fs::write(
        &manifest,
        original.replace("target_triple = \"x86_64-pc-windows-msvc\"", "target_triple = \"x86_64-unknown-linux-gnu\""),
    )
    .expect("rewrite manifest");
    let result = run_corvid(
        &[
            "bundle",
            "verify",
            fixture.root.to_str().expect("utf8 root"),
            "--rebuild",
        ],
        &fixture.root,
    );
    assert_eq!(result.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("BundlePlatformUnsupported"), "stderr was: {stderr}");
}
