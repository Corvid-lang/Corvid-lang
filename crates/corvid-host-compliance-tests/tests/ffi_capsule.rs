use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .to_path_buf()
}

fn compliance_target_dir() -> PathBuf {
    workspace_root().join("target").join("host-compliance")
}

fn shared_library_name(stem: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("lib{stem}.dylib")
    } else if cfg!(windows) {
        format!("{stem}.dll")
    } else {
        format!("lib{stem}.so")
    }
}

fn run_corvid(args: &[&str], cwd: &Path) -> std::process::Output {
    Command::new("cargo")
        .env("CARGO_TARGET_DIR", compliance_target_dir())
        .arg("run")
        .arg("-q")
        .arg("-p")
        .arg("corvid-cli")
        .arg("--")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run corvid")
}

fn test_tools_lib_path() -> PathBuf {
    let root = workspace_root();
    let target_dir = compliance_target_dir();
    let status = Command::new("cargo")
        .env("CARGO_TARGET_DIR", &target_dir)
        .arg("build")
        .arg("-p")
        .arg("corvid-test-tools")
        .arg("--release")
        .current_dir(&root)
        .status()
        .expect("build corvid-test-tools");
    assert!(status.success(), "building corvid-test-tools failed");
    let path = if cfg!(windows) {
        target_dir.join("release").join("corvid_test_tools.lib")
    } else {
        target_dir.join("release").join("libcorvid_test_tools.a")
    };
    // Route the linker through `corvid_test_tools.lib` (which already
    // bundles `corvid-runtime` transitively) instead of pairing it
    // with the standalone `corvid_runtime.lib`. See
    // `corvid-codegen-cl::cdylib::runtime_staticlib_path`.
    unsafe {
        std::env::set_var("CORVID_RUNTIME_STATICLIB_OVERRIDE", &path);
    }
    path
}

fn try_compiler() -> Option<cc::Tool> {
    cc::Build::new()
        .opt_level(0)
        .cargo_metadata(false)
        .cargo_warnings(false)
        .host(&target_lexicon::HOST.to_string())
        .target(&target_lexicon::HOST.to_string())
        .try_get_compiler()
        .ok()
}

fn compile_host(source: &Path, include_dir: &Path, out_dir: &Path) -> Option<PathBuf> {
    let compiler = match try_compiler() {
        Some(compiler) => compiler,
        None => {
            eprintln!("skipping: no C compiler on PATH");
            return None;
        }
    };
    let output_stem = source.file_stem()?.to_str()?;
    let output_path = if cfg!(windows) {
        out_dir.join(format!("{output_stem}.exe"))
    } else {
        out_dir.join(output_stem)
    };
    let mut cmd = Command::new(compiler.path());
    for (key, value) in compiler.env() {
        cmd.env(key, value);
    }
    if compiler.is_like_msvc() {
        cmd.arg(source)
            .arg(format!("/I{}", include_dir.display()))
            .arg(format!("/Fe:{}", output_path.display()));
    } else {
        cmd.arg(source)
            .arg("-I")
            .arg(include_dir)
            .arg("-Wall")
            .arg("-Wextra")
            .arg("-Werror")
            .arg("-ldl")
            .arg("-o")
            .arg(&output_path);
    }
    let output = cmd.output().expect("compile C host");
    assert!(
        output.status.success(),
        "C host compile failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Some(output_path)
}

fn find_python() -> Option<Vec<String>> {
    let candidates = if cfg!(windows) {
        vec![
            vec!["py".to_string(), "-3".to_string()],
            vec!["python".to_string()],
        ]
    } else {
        vec![vec!["python3".to_string()], vec!["python".to_string()]]
    };
    for candidate in candidates {
        let mut cmd = Command::new(&candidate[0]);
        for arg in &candidate[1..] {
            cmd.arg(arg);
        }
        if cmd.arg("--version").output().map(|out| out.status.success()).unwrap_or(false) {
            return Some(candidate);
        }
    }
    None
}

#[test]
fn c_recording_replays_from_python_and_bad_json_is_not_silent() {
    let root = workspace_root();
    let demo_dir = root.join("examples").join("cdylib_catalog_demo");
    let source = demo_dir.join("src").join("classify.cor");
    let tools_lib = test_tools_lib_path();

    let build_output = run_corvid(
        &[
            "build",
            source.to_str().expect("utf8 source path"),
            "--target=cdylib",
            "--with-tools-lib",
            tools_lib.to_str().expect("utf8 tools path"),
            "--all-artifacts",
        ],
        &root,
    );
    assert!(
        build_output.status.success(),
        "cdylib demo build failed: stdout={} stderr={}",
        String::from_utf8_lossy(&build_output.stdout),
        String::from_utf8_lossy(&build_output.stderr)
    );

    let hash_output = run_corvid(
        &["abi", "hash", source.to_str().expect("utf8 source path")],
        &root,
    );
    assert!(
        hash_output.status.success(),
        "abi hash failed: stdout={} stderr={}",
        String::from_utf8_lossy(&hash_output.stdout),
        String::from_utf8_lossy(&hash_output.stderr)
    );
    let hash = String::from_utf8(hash_output.stdout).expect("hash utf8").trim().to_string();

    let release_dir = demo_dir.join("target").join("release");
    let library = release_dir.join(shared_library_name("classify"));
    let host_source = demo_dir.join("host_c").join("capsule_host.c");
    let host_bin = match compile_host(&host_source, &release_dir, &demo_dir.join("host_c")) {
        Some(path) => path,
        None => return,
    };
    let trace_dir = demo_dir.join("trace_output");
    std::fs::create_dir_all(&trace_dir).expect("create trace_output");
    let trace_path = trace_dir.join("host_compliance.jsonl");

    let c_output = Command::new(&host_bin)
        .arg(&library)
        .arg(&hash)
        .arg(&trace_path)
        .current_dir(&demo_dir)
        .output()
        .expect("run C compliance host");
    assert!(
        c_output.status.success(),
        "C host failed: stdout={} stderr={}",
        String::from_utf8_lossy(&c_output.stdout),
        String::from_utf8_lossy(&c_output.stderr)
    );
    let c_stdout = String::from_utf8_lossy(&c_output.stdout);
    assert!(c_stdout.contains("host_event_status=0"), "stdout was: {c_stdout}");
    assert!(c_stdout.contains("bad_json_status=1"), "stdout was: {c_stdout}");
    let c_replay_line = c_stdout
        .lines()
        .find(|line| line.starts_with("replay_line="))
        .expect("C host replay_line")
        .to_string();

    let python = match find_python() {
        Some(python) => python,
        None => {
            eprintln!("skipping: no python interpreter on PATH");
            return;
        }
    };
    let replay_script = demo_dir.join("host_py").join("replay_host.py");
    let mut cmd = Command::new(&python[0]);
    for arg in &python[1..] {
        cmd.arg(arg);
    }
    let py_output = cmd
        .arg(&replay_script)
        .arg(&library)
        .arg(&trace_path)
        .current_dir(&demo_dir)
        .output()
        .expect("run python replay host");
    assert!(
        py_output.status.success(),
        "Python host failed: stdout={} stderr={}",
        String::from_utf8_lossy(&py_output.stdout),
        String::from_utf8_lossy(&py_output.stderr)
    );
    let py_stdout = String::from_utf8_lossy(&py_output.stdout);
    let py_replay_line = py_stdout
        .lines()
        .find(|line| line.starts_with("replay_line="))
        .expect("Python host replay_line")
        .to_string();
    assert_eq!(c_replay_line, py_replay_line);

    let trace = std::fs::read_to_string(&trace_path).expect("read trace");
    assert!(trace.contains("\"kind\":\"host_event\""), "trace was: {trace}");
    assert!(trace.contains("\"name\":\"capsule_record\""), "trace was: {trace}");
    assert!(!trace.contains("\"name\":\"bad_json\""), "trace was: {trace}");
}
