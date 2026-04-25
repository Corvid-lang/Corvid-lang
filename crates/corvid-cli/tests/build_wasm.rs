use std::path::{Path, PathBuf};
use std::process::Command;

fn write_project(src: &str, stem: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let src_dir = dir.path().join("src");
    std::fs::create_dir_all(&src_dir).expect("create src dir");
    let source_path = src_dir.join(format!("{stem}.cor"));
    std::fs::write(&source_path, src).expect("write source");
    (dir, source_path)
}

fn run_corvid(args: &[&str], cwd: &Path) -> std::process::Output {
    let exe = env!("CARGO_BIN_EXE_corvid");
    Command::new(exe)
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run corvid")
}

#[test]
fn cli_build_wasm_emits_module_loader_types_and_manifest() {
    let (dir, source_path) = write_project(
        r#"
agent add_one(x: Int) -> Int:
    y = x + 1
    return y
"#,
        "math",
    );

    let output = run_corvid(
        &["build", source_path.to_str().unwrap(), "--target=wasm"],
        dir.path(),
    );
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let out_dir = dir.path().join("target").join("wasm");
    let wasm_path = out_dir.join("math.wasm");
    let js_path = out_dir.join("math.js");
    let ts_path = out_dir.join("math.d.ts");
    let manifest_path = out_dir.join("math.corvid-wasm.json");

    let wasm = std::fs::read(&wasm_path).expect("wasm");
    wasmparser::Validator::new()
        .validate_all(&wasm)
        .expect("valid wasm");

    let js = std::fs::read_to_string(js_path).expect("js loader");
    assert!(js.contains("WebAssembly.instantiateStreaming"));
    assert!(js.contains("add_one(x)"));

    let ts = std::fs::read_to_string(ts_path).expect("ts types");
    assert!(ts.contains("add_one(x: bigint): bigint"));

    let manifest = std::fs::read_to_string(manifest_path).expect("manifest");
    assert!(manifest.contains("\"target\": \"wasm32-unknown-unknown\""));
    assert!(manifest.contains("\"host_capability_abi\""));
}

#[test]
fn cli_build_wasm_emits_prompt_host_imports() {
    let (dir, source_path) = write_project(
        r#"
prompt answer() -> Int:
    """Return 42."""

agent main() -> Int:
    return answer()
"#,
        "prompted",
    );

    let output = run_corvid(
        &["build", source_path.to_str().unwrap(), "--target=wasm"],
        dir.path(),
    );
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let out_dir = dir.path().join("target").join("wasm");
    let wasm = std::fs::read(out_dir.join("prompted.wasm")).expect("wasm");
    wasmparser::Validator::new()
        .validate_all(&wasm)
        .expect("valid wasm");
    let manifest =
        std::fs::read_to_string(out_dir.join("prompted.corvid-wasm.json")).expect("manifest");
    assert!(manifest.contains("\"import_name\": \"prompt.answer\""));
    let types = std::fs::read_to_string(out_dir.join("prompted.d.ts")).expect("types");
    assert!(types.contains("'answer': () => bigint"));
    assert!(types.contains("CorvidWasmTraceSink"));
    let js = std::fs::read_to_string(out_dir.join("prompted.js")).expect("loader");
    assert!(js.contains("kind: 'llm_call'"));
    assert!(js.contains("kind: 'run_completed'"));
}

#[test]
fn committed_wasm_browser_demo_builds_and_uses_generated_loader() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf();
    let demo = root.join("examples").join("wasm_browser_demo");
    let source = demo.join("src").join("refund_gate.cor");
    let output = run_corvid(
        &["build", source.to_str().unwrap(), "--target=wasm"],
        &root,
    );
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let out_dir = demo.join("target").join("wasm");
    let wasm = std::fs::read(out_dir.join("refund_gate.wasm")).expect("wasm");
    wasmparser::Validator::new()
        .validate_all(&wasm)
        .expect("valid wasm");

    let loader = std::fs::read_to_string(out_dir.join("refund_gate.js")).expect("loader");
    assert!(loader.contains("kind: 'approval_decision'"));
    assert!(loader.contains("kind: 'tool_result'"));

    let types = std::fs::read_to_string(out_dir.join("refund_gate.d.ts")).expect("types");
    assert!(types.contains("review_refund(amount: bigint): bigint"));
    assert!(types.contains("'IssueRefund': (arg1: bigint) => boolean"));

    let page = std::fs::read_to_string(demo.join("web").join("index.html")).expect("page");
    assert!(page.contains("demo.js"));
    let browser_host = std::fs::read_to_string(demo.join("web").join("demo.js")).expect("host");
    assert!(browser_host.contains("../target/wasm/refund_gate.js"));
    assert!(browser_host.contains("approvals"));
    assert!(browser_host.contains("trace"));
}
