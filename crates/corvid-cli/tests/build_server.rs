use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const SOURCE: &str = r#"
agent main() -> String:
    return "hello from corvid"
"#;

fn corvid_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_corvid"))
}

fn server_binary_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}_server.exe")
    } else {
        format!("{stem}_server")
    }
}

fn run_corvid(args: &[String], cwd: &Path) -> std::process::Output {
    Command::new(corvid_bin())
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run corvid")
}

fn http_request(addr: &str, request: &str) -> String {
    let mut stream = TcpStream::connect(addr).expect("connect server");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    stream
        .write_all(request.as_bytes())
        .expect("write request");
    let mut bytes = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => bytes.extend_from_slice(&buf[..n]),
            Err(err) if !bytes.is_empty() => {
                eprintln!("server closed connection after partial response: {err}");
                break;
            }
            Err(err) => panic!("read response: {err}"),
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn http_get(addr: &str, path: &str) -> String {
    http_request(
        addr,
        &format!("GET {path} HTTP/1.1\r\nhost: {addr}\r\nconnection: close\r\n\r\n"),
    )
}

struct ChildGuard(std::process::Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn build_server_emits_runnable_local_http_binary() {
    let dir = tempfile::tempdir().expect("tempdir");
    let src_dir = dir.path().join("src");
    std::fs::create_dir_all(&src_dir).expect("src dir");
    let source_path = src_dir.join("hello.cor");
    std::fs::write(&source_path, SOURCE).expect("write source");

    let args = vec![
        "build".to_string(),
        source_path.to_string_lossy().into_owned(),
        "--target=server".to_string(),
    ];
    let out = run_corvid(&args, dir.path());
    assert!(
        out.status.success(),
        "server build failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let server = dir
        .path()
        .join("target")
        .join("server")
        .join(server_binary_name("hello"));
    assert!(server.exists(), "missing server binary at {}", server.display());

    let child = Command::new(&server)
        .env("CORVID_PORT", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn server");
    let mut child = ChildGuard(child);
    let stdout = child.0.stdout.take().expect("server stdout");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    let start = Instant::now();
    while line.is_empty() && start.elapsed() < Duration::from_secs(10) {
        reader.read_line(&mut line).expect("read listening line");
    }
    assert!(
        line.starts_with("listening: http://"),
        "unexpected server stdout line: {line:?}"
    );
    let addr = line
        .trim()
        .strip_prefix("listening: http://")
        .expect("listening prefix");

    let health = http_get(addr, "/healthz");
    assert!(health.contains("HTTP/1.1 200 OK"), "{health}");
    assert!(health.contains(r#"{"status":"ok"}"#), "{health}");
    assert!(health.contains("x-corvid-request-id:"), "{health}");

    let ready = http_get(addr, "/readyz");
    assert!(ready.contains("HTTP/1.1 200 OK"), "{ready}");
    assert!(ready.contains(r#"{"ready":true}"#), "{ready}");

    let root = http_get(addr, "/");
    assert!(root.contains("HTTP/1.1 200 OK"), "{root}");
    assert!(root.contains(r#""result":"hello from corvid""#), "{root}");

    let rejected = http_request(
        addr,
        &format!("POST / HTTP/1.1\r\nhost: {addr}\r\nconnection: close\r\n\r\n"),
    );
    assert!(
        rejected.contains("HTTP/1.1 405 Method Not Allowed"),
        "{rejected}"
    );
    assert!(rejected.contains("content-type: application/json"), "{rejected}");
    assert!(rejected.contains("x-corvid-request-id: req-"), "{rejected}");
    assert!(rejected.contains(r#""request_id":"req-"#), "{rejected}");
    assert!(rejected.contains(r#""route":"/""#), "{rejected}");
    assert!(
        rejected.contains(r#""kind":"method_not_allowed""#),
        "{rejected}"
    );
    assert!(
        rejected.contains(r#""message":"method not allowed""#),
        "{rejected}"
    );

    let query = http_get(addr, "/?source=parser");
    assert!(query.contains("HTTP/1.1 200 OK"), "{query}");
    assert!(query.contains(r#""result":"hello from corvid""#), "{query}");

    let oversized = http_request(
        addr,
        &format!(
            "POST / HTTP/1.1\r\nhost: {addr}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            4097
        ),
    );
    assert!(
        oversized.contains("HTTP/1.1 413 Payload Too Large"),
        "{oversized}"
    );
    assert!(
        oversized.contains(r#""kind":"body_too_large""#),
        "{oversized}"
    );

    let metrics = http_get(addr, "/metrics");
    assert!(metrics.contains("HTTP/1.1 200 OK"), "{metrics}");
    assert!(metrics.contains(r#""request_total":"#), "{metrics}");
    assert!(metrics.contains(r#""error_total":"#), "{metrics}");
    assert!(metrics.contains(r#""runtime":"corvid-server""#), "{metrics}");

    drop(child);

    let timeout_child = Command::new(&server)
        .env("CORVID_PORT", "0")
        .env("CORVID_HANDLER_TIMEOUT_MS", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn timeout server");
    let mut timeout_child = ChildGuard(timeout_child);
    let stdout = timeout_child.0.stdout.take().expect("timeout server stdout");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    let start = Instant::now();
    while line.is_empty() && start.elapsed() < Duration::from_secs(10) {
        reader
            .read_line(&mut line)
            .expect("read timeout listening line");
    }
    let timeout_addr = line
        .trim()
        .strip_prefix("listening: http://")
        .expect("timeout listening prefix");
    let timeout = http_get(timeout_addr, "/");
    assert!(
        timeout.contains("HTTP/1.1 504 Gateway Timeout"),
        "{timeout}"
    );
    assert!(timeout.contains(r#""kind":"handler_timeout""#), "{timeout}");

    drop(timeout_child);

    let invalid_config = Command::new(&server)
        .env("CORVID_PORT", "0")
        .env("CORVID_HANDLER_TIMEOUT_MS", "super-secret-invalid")
        .output()
        .expect("run invalid config server");
    assert!(!invalid_config.status.success());
    let invalid_stderr = String::from_utf8_lossy(&invalid_config.stderr);
    assert!(
        invalid_stderr.contains("CORVID_HANDLER_TIMEOUT_MS invalid"),
        "{invalid_stderr}"
    );
    assert!(invalid_stderr.contains("value redacted"), "{invalid_stderr}");
    assert!(
        !invalid_stderr.contains("super-secret-invalid"),
        "{invalid_stderr}"
    );

    let doctor = Command::new(corvid_bin())
        .arg("doctor")
        .env("CORVID_HANDLER_TIMEOUT_MS", "super-secret-invalid")
        .output()
        .expect("run doctor");
    assert!(!doctor.status.success());
    let doctor_stdout = String::from_utf8_lossy(&doctor.stdout);
    assert!(
        doctor_stdout.contains("CORVID_HANDLER_TIMEOUT_MS invalid"),
        "{doctor_stdout}"
    );
    assert!(doctor_stdout.contains("value redacted"), "{doctor_stdout}");
    assert!(
        !doctor_stdout.contains("super-secret-invalid"),
        "{doctor_stdout}"
    );

    let trace_child = Command::new(&server)
        .env("CORVID_PORT", "0")
        .env("CORVID_MAX_REQUESTS", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn trace server");
    let mut trace_child = ChildGuard(trace_child);
    let stdout = trace_child.0.stdout.take().expect("trace server stdout");
    let mut stderr = trace_child.0.stderr.take().expect("trace server stderr");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    let start = Instant::now();
    while line.is_empty() && start.elapsed() < Duration::from_secs(10) {
        reader
            .read_line(&mut line)
            .expect("read trace listening line");
    }
    let trace_addr = line
        .trim()
        .strip_prefix("listening: http://")
        .expect("trace listening prefix");
    let traced = http_get(trace_addr, "/healthz");
    assert!(traced.contains("HTTP/1.1 200 OK"), "{traced}");
    let _ = trace_child.0.wait();
    let mut traces = String::new();
    stderr.read_to_string(&mut traces).expect("read traces");
    assert!(
        traces.contains(r#""event":"corvid.server.request""#),
        "{traces}"
    );
    assert!(traces.contains(r#""method":"GET""#), "{traces}");
    assert!(traces.contains(r#""route":"/healthz""#), "{traces}");
    assert!(traces.contains(r#""status":200"#), "{traces}");
    assert!(traces.contains(r#""effects":[]"#), "{traces}");
}

#[test]
fn refund_api_backend_example_checks_and_builds() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let example = repo.join("examples").join("backend").join("refund_api");

    let contract = example.join("src").join("refund_api.cor");
    let check = run_corvid(
        &["check".to_string(), contract.to_string_lossy().into_owned()],
        &repo,
    );
    assert!(
        check.status.success(),
        "refund contract check failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    );

    let entrypoint = example.join("src").join("main.cor");
    let build = run_corvid(
        &[
            "build".to_string(),
            entrypoint.to_string_lossy().into_owned(),
            "--target=server".to_string(),
        ],
        &repo,
    );
    assert!(
        build.status.success(),
        "refund server build failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
}
