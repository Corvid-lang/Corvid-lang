use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

fn demo_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn read_text(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()))
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

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .to_path_buf()
}

fn run_corvid(args: &[&str], cwd: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_corvid"))
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run corvid")
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

fn test_tools_lib_path() -> PathBuf {
    let root = workspace_root();
    let status = Command::new("cargo")
        .arg("build")
        .arg("-p")
        .arg("corvid-test-tools")
        .arg("--release")
        .current_dir(&root)
        .status()
        .expect("build corvid-test-tools");
    assert!(status.success(), "building corvid-test-tools failed");
    let path = if cfg!(windows) {
        root.join("target").join("release").join("corvid_test_tools.lib")
    } else {
        root.join("target").join("release").join("libcorvid_test_tools.a")
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
    let output_stem = source
        .file_stem()
        .and_then(|stem| stem.to_str())
        .expect("host source stem");
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
    let output = cmd.output().expect("compile approver host");
    assert!(
        output.status.success(),
        "approver host compile failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Some(output_path)
}

#[test]
fn cdylib_catalog_demo_c_host_shows_accept_reject_and_fail_closed() {
    let _guard = demo_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = workspace_root();
    let demo_dir = root.join("examples").join("cdylib_catalog_demo");
    let source = demo_dir.join("src").join("classify.cor");
    let approver = demo_dir.join("src").join("approver.cor");
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
    let hash = String::from_utf8(hash_output.stdout)
        .expect("hash utf8")
        .trim()
        .to_string();

    let release_dir = demo_dir.join("target").join("release");
    let library = release_dir.join(shared_library_name("classify"));
    let host_source = demo_dir.join("host_c").join("approver_host.c");
    let trace_dir = demo_dir.join("trace_output");
    let trace_path = trace_dir.join("approval_demo.jsonl");
    let host_bin = match compile_host(&host_source, &release_dir, &demo_dir.join("host_c")) {
        Some(path) => path,
        None => return,
    };
    let _ = std::fs::remove_dir_all(&trace_dir);

    let output = Command::new(&host_bin)
        .arg(&library)
        .arg(&approver)
        .arg(&hash)
        .current_dir(&demo_dir)
        .output()
        .expect("run approver host");
    assert!(
        output.status.success(),
        "approver host failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("verified_before=1"), "stdout was: {stdout}");
    assert!(stdout.contains("verified_after_registration=1"), "stdout was: {stdout}");
    assert!(stdout.contains("catalog_has_approver=1"), "stdout was: {stdout}");
    assert!(stdout.contains("preflight_status=0 requires_approval=1"), "stdout was: {stdout}");
    assert!(stdout.contains("accept_call_status=0 result=\"approved\""), "stdout was: {stdout}");
    assert!(stdout.contains("reject_call_status=4 site=EchoString"), "stdout was: {stdout}");
    assert!(stdout.contains("fail_closed_call_status=4 site=EchoString"), "stdout was: {stdout}");
    assert!(stdout.contains("trace_path=trace_output/approval_demo.jsonl"), "stdout was: {stdout}");

    let trace = read_text(&trace_path);
    assert!(trace.contains("\"kind\":\"approval_decision\""), "trace was: {trace}");
    assert!(trace.contains("\"decider\":\"corvid-agent:"), "trace was: {trace}");
    assert!(trace.contains("\"accepted\":true"), "trace was: {trace}");
    assert!(trace.contains("\"accepted\":false"), "trace was: {trace}");
    assert!(trace.contains("\"decider\":\"fail-closed-default\""), "trace was: {trace}");
}

#[test]
fn cdylib_catalog_demo_filter_host_narrows_catalog() {
    let _guard = demo_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
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
    let hash = String::from_utf8(hash_output.stdout)
        .expect("hash utf8")
        .trim()
        .to_string();

    let release_dir = demo_dir.join("target").join("release");
    let library = release_dir.join(shared_library_name("classify"));
    let host_source = demo_dir.join("host_c").join("host.c");
    let host_bin = match compile_host(&host_source, &release_dir, &demo_dir.join("host_c")) {
        Some(path) => path,
        None => return,
    };

    let output = Command::new(&host_bin)
        .arg(&library)
        .arg(&hash)
        .arg("--filter={\"all\":[{\"dim\":\"trust_tier\",\"op\":\"le\",\"value\":\"autonomous\"},{\"dim\":\"dangerous\",\"op\":\"eq\",\"value\":false}]}")
        .current_dir(&demo_dir)
        .output()
        .expect("run filter host");
    assert!(
        output.status.success(),
        "filter host failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("verified=1"), "stdout was: {stdout}");
    assert!(stdout.contains("filter_status=0"), "stdout was: {stdout}");
    assert!(stdout.contains("filtered_count="), "stdout was: {stdout}");
    assert!(stdout.contains("filtered_agent=classify"), "stdout was: {stdout}");
    assert!(stdout.contains("observation_handle="), "stdout was: {stdout}");
    assert!(stdout.contains("cost_usd="), "stdout was: {stdout}");
    assert!(stdout.contains("latency_ms="), "stdout was: {stdout}");
    assert!(stdout.contains("tokens_in="), "stdout was: {stdout}");
    assert!(stdout.contains("tokens_out="), "stdout was: {stdout}");
    assert!(stdout.contains("exceeded_bound=0"), "stdout was: {stdout}");
    assert!(stdout.contains("grounded_result=catalog-proof"), "stdout was: {stdout}");
    assert!(stdout.contains("grounded_handle="), "stdout was: {stdout}");
    assert!(stdout.contains("grounded_source_count=1"), "stdout was: {stdout}");
    assert!(stdout.contains("grounded_source=grounded_echo"), "stdout was: {stdout}");
    assert!(stdout.contains("grounded_confidence=1.00"), "stdout was: {stdout}");
}

#[test]
fn cdylib_catalog_demo_capsule_create_and_replay_work() {
    let _guard = demo_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
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
    let hash = String::from_utf8(hash_output.stdout)
        .expect("hash utf8")
        .trim()
        .to_string();

    let release_dir = demo_dir.join("target").join("release");
    let library = release_dir.join(shared_library_name("classify"));
    let host_source = demo_dir.join("host_c").join("capsule_host.c");
    let host_bin = match compile_host(&host_source, &release_dir, &demo_dir.join("host_c")) {
        Some(path) => path,
        None => return,
    };
    let trace_dir = demo_dir.join("trace_output");
    std::fs::create_dir_all(&trace_dir).expect("create trace_output");
    let trace_path = trace_dir.join("capsule_demo.jsonl");
    let capsule_path = trace_dir.join("capsule_demo.capsule");

    let host_output = Command::new(&host_bin)
        .arg(&library)
        .arg(&hash)
        .arg(&trace_path)
        .current_dir(&demo_dir)
        .output()
        .expect("run capsule host");
    assert!(
        host_output.status.success(),
        "capsule host failed: stdout={} stderr={}",
        String::from_utf8_lossy(&host_output.stdout),
        String::from_utf8_lossy(&host_output.stderr)
    );
    let host_stdout = String::from_utf8_lossy(&host_output.stdout);
    assert!(host_stdout.contains("verified=1"), "stdout was: {host_stdout}");
    assert!(host_stdout.contains("host_event_status=0"), "stdout was: {host_stdout}");
    assert!(host_stdout.contains("bad_json_status=1"), "stdout was: {host_stdout}");
    assert!(host_stdout.contains("replay_line=status=0 result=\"positive\""), "stdout was: {host_stdout}");

    let trace = read_text(&trace_path);
    assert!(trace.contains("\"kind\":\"host_event\""), "trace was: {trace}");
    assert!(trace.contains("\"name\":\"capsule_record\""), "trace was: {trace}");
    assert!(!trace.contains("\"name\":\"bad_json\""), "trace was: {trace}");

    let create_output = run_corvid(
        &[
            "capsule",
            "create",
            trace_path.to_str().expect("utf8 trace"),
            library.to_str().expect("utf8 library"),
            "--out",
            capsule_path.to_str().expect("utf8 capsule"),
        ],
        &root,
    );
    assert!(
        create_output.status.success(),
        "capsule create failed: stdout={} stderr={}",
        String::from_utf8_lossy(&create_output.stdout),
        String::from_utf8_lossy(&create_output.stderr)
    );

    let replay_output = run_corvid(
        &["capsule", "replay", capsule_path.to_str().expect("utf8 capsule")],
        &root,
    );
    assert!(
        replay_output.status.success(),
        "capsule replay failed: stdout={} stderr={}",
        String::from_utf8_lossy(&replay_output.stdout),
        String::from_utf8_lossy(&replay_output.stderr)
    );
    let replay_stdout = String::from_utf8_lossy(&replay_output.stdout);
    assert!(
        replay_stdout.contains("agent=classify status=0 result=\"positive\""),
        "stdout was: {replay_stdout}"
    );

    let python = match find_python() {
        Some(python) => python,
        None => {
            eprintln!("skipping python replay check: no python on PATH");
            return;
        }
    };
    let replay_script = demo_dir.join("host_py").join("replay_host.py");
    let mut cmd = Command::new(&python[0]);
    for arg in &python[1..] {
        cmd.arg(arg);
    }
    let python_output = cmd
        .arg(&replay_script)
        .arg(&library)
        .arg(&trace_path)
        .current_dir(&demo_dir)
        .output()
        .expect("run python replay host");
    assert!(
        python_output.status.success(),
        "python replay host failed: stdout={} stderr={}",
        String::from_utf8_lossy(&python_output.stdout),
        String::from_utf8_lossy(&python_output.stderr)
    );
    let python_stdout = String::from_utf8_lossy(&python_output.stdout);
    assert!(
        python_stdout.contains("replay_line=status=0 result=\"positive\""),
        "stdout was: {python_stdout}"
    );
}
