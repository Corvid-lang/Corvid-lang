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

fn http_get(addr: &str, path: &str) -> String {
    let mut stream = TcpStream::connect(addr).expect("connect server");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nhost: {addr}\r\nconnection: close\r\n\r\n"
    )
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

    let root = http_get(addr, "/");
    assert!(root.contains("HTTP/1.1 200 OK"), "{root}");
    assert!(root.contains(r#""result":"hello from corvid""#), "{root}");

    drop(child);
}
