//! Generated-server source rendering.
//!
//! `corvid build --target server` emits a Rust crate (Cargo.toml +
//! src/main.rs) wrapping the user's Corvid program in either an
//! Axum HTTP server (when at least one `route` decl is present) or
//! a minimal `TcpListener`-based handler (otherwise). This module
//! owns those template renderers plus the small naming helpers
//! that pick the package, binary, and crate-root paths.

use std::path::{Path, PathBuf};

pub(super) fn server_binary_path_for(out_dir: &Path, stem: &str) -> PathBuf {
    if cfg!(windows) {
        out_dir.join(format!("{stem}_server.exe"))
    } else {
        out_dir.join(format!("{stem}_server"))
    }
}

pub(super) fn server_binary_name_for_package(package: &str) -> String {
    if cfg!(windows) {
        format!("{package}.exe")
    } else {
        package.to_string()
    }
}

pub(super) fn server_package_name(stem: &str) -> String {
    let mut out = String::from("corvid_generated_");
    for ch in stem.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    out
}

pub(super) fn render_server_cargo_toml(package: &str) -> String {
    format!(
        r#"[package]
name = "{package}"
version = "0.0.0"
edition = "2021"

[workspace]

[dependencies]
axum = "0.7"
tokio = {{ version = "1", features = ["full"] }}
tower-http = {{ version = "0.6", features = ["compression-full", "cors", "trace"] }}
"#
    )
}

pub(super) fn render_axum_server_source(handler_path: &Path) -> String {
    let handler = handler_path.to_string_lossy().replace('\\', "\\\\");
    format!(
        r#"use axum::extract::State;
use axum::http::{{HeaderValue, Method, Request, StatusCode}};
use axum::middleware::Next;
use axum::response::{{IntoResponse, Response}};
use axum::routing::get;
use axum::middleware;
use axum::Router;
use std::io::Read;
use std::process::{{Command, Stdio}};
use std::sync::atomic::{{AtomicU64, Ordering}};
use std::sync::{{Arc, Mutex}};
use std::time::{{Duration, Instant, SystemTime, UNIX_EPOCH}};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tower_http::compression::CompressionLayer;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

const HANDLER: &str = "{handler}";
const MAX_REQUEST_BYTES: usize = 4096;
static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);
static REQUEST_TOTAL: AtomicU64 = AtomicU64::new(0);
static ERROR_TOTAL: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
struct AppState {{
    max_requests: Option<u64>,
    require_auth: bool,
    rate_limit_requests: Option<u64>,
    rate_limit_seen: Arc<AtomicU64>,
    handled_requests: Arc<AtomicU64>,
    shutdown: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}}

#[tokio::main]
async fn main() -> std::io::Result<()> {{
    let host = std::env::var("CORVID_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = std::env::var("CORVID_PORT").unwrap_or_else(|_| "8080".to_string());
    validate_runtime_config()?;
    let listener = TcpListener::bind(format!("{{host}}:{{port}}")).await?;
    let addr = listener.local_addr()?;
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let state = AppState {{
        max_requests: max_requests(),
        require_auth: require_auth(),
        rate_limit_requests: rate_limit_requests(),
        rate_limit_seen: Arc::new(AtomicU64::new(0)),
        handled_requests: Arc::new(AtomicU64::new(0)),
        shutdown: Arc::new(Mutex::new(Some(shutdown_tx))),
    }};
    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .fallback(handle_app)
        .layer(middleware::from_fn_with_state(state.clone(), backend_middleware))
        .layer(CompressionLayer::new())
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state);
    println!("listening: http://{{addr}}");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {{
            let _ = shutdown_rx.await;
        }})
        .await
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err.to_string()))?;
    Ok(())
}}

async fn backend_middleware(
    State(state): State<AppState>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {{
    let started = Instant::now();
    let request_id = request_id();
    let method = request.method().as_str().to_string();
    let path = request.uri().path().to_string();
    if state.require_auth
        && request
            .headers()
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .filter(|value| value.starts_with("Bearer "))
            .is_none()
    {{
        return error_response(
            state,
            401,
            &method,
            &path,
            "auth_required",
            "authorization bearer token required",
            request_id,
            started,
        );
    }}
    if let Some(limit) = state.rate_limit_requests {{
        let seen = state.rate_limit_seen.fetch_add(1, Ordering::Relaxed) + 1;
        if seen > limit {{
            return error_response(
                state,
                429,
                &method,
                &path,
                "rate_limited",
                "request rate limit exceeded",
                request_id,
                started,
            );
        }}
    }}
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        "x-corvid-middleware",
        HeaderValue::from_static("auth,rate_limit,tracing,cors,compression,request_logging,effect_policy"),
    );
    headers.insert("x-corvid-effect-policy", HeaderValue::from_static("enforced"));
    response
}}

async fn healthz(State(state): State<AppState>, request: Request<axum::body::Body>) -> Response {{
    complete(
        state,
        "GET",
        request.uri().path(),
        200,
        "application/json",
        "{{\"status\":\"ok\"}}".to_string(),
        request_id(),
        Instant::now(),
    )
}}

async fn readyz(State(state): State<AppState>, request: Request<axum::body::Body>) -> Response {{
    complete(
        state,
        "GET",
        request.uri().path(),
        200,
        "application/json",
        "{{\"ready\":true}}".to_string(),
        request_id(),
        Instant::now(),
    )
}}

async fn metrics(State(state): State<AppState>, request: Request<axum::body::Body>) -> Response {{
    let body = format!(
        "{{{{\"request_total\":{{}},\"error_total\":{{}},\"runtime\":\"corvid-server\"}}}}",
        REQUEST_TOTAL.load(Ordering::Relaxed),
        ERROR_TOTAL.load(Ordering::Relaxed)
    );
    complete(
        state,
        "GET",
        request.uri().path(),
        200,
        "application/json",
        body,
        request_id(),
        Instant::now(),
    )
}}

async fn handle_app(
    State(state): State<AppState>,
    method: Method,
    request: Request<axum::body::Body>,
) -> Response {{
    let started = Instant::now();
    let request_id = request_id();
    let method_text = method.as_str().to_string();
    let path = request.uri().path().to_string();
    let content_length = request
        .headers()
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|header| header.to_str().ok())
        .and_then(|header| header.parse::<usize>().ok())
        .unwrap_or(0);
    if content_length > MAX_REQUEST_BYTES {{
        return error_response(
            state,
            413,
            &method_text,
            &path,
            "body_too_large",
            "request exceeds server body limit",
            request_id,
            started,
        );
    }}
    if method != Method::GET {{
        return error_response(
            state,
            405,
            &method_text,
            &path,
            "method_not_allowed",
            "method not allowed",
            request_id,
            started,
        );
    }}
    let output = run_handler(handler_timeout());
    match output {{
        Ok(out) if out.status_success => {{
            let body = out.stdout.trim().to_string();
            let json = format!("{{{{\"result\":{{:?}}}}}}", body);
            complete(state, &method_text, &path, 200, "application/json", json, request_id, started)
        }}
        Ok(out) => {{
            let err = out.stderr.trim().to_string();
            error_response(
                state,
                500,
                &method_text,
                &path,
                "handler_failed",
                if err.is_empty() {{ "handler failed" }} else {{ &err }},
                request_id,
                started,
            )
        }}
        Err(HandlerError::TimedOut) => error_response(
            state,
            504,
            &method_text,
            &path,
            "handler_timeout",
            "handler timed out",
            request_id,
            started,
        ),
        Err(HandlerError::Spawn(err)) => error_response(
            state,
            500,
            &method_text,
            &path,
            "handler_spawn_failed",
            &err,
            request_id,
            started,
        ),
    }}
}}

fn error_response(
    state: AppState,
    status: u16,
    method: &str,
    route: &str,
    kind: &str,
    message: &str,
    request_id: String,
    started: Instant,
) -> Response {{
    let body = format!(
        "{{{{\"request_id\":{{}},\"route\":{{}},\"kind\":{{}},\"message\":{{}},\"duration_ms\":{{}}}}}}",
        json_string(&request_id),
        json_string(route),
        json_string(kind),
        json_string(message),
        started.elapsed().as_millis()
    );
    complete(state, method, route, status, "application/json", body, request_id, started)
}}

fn complete(
    state: AppState,
    method: &str,
    route: &str,
    status: u16,
    content_type: &str,
    body: String,
    request_id: String,
    started: Instant,
) -> Response {{
    REQUEST_TOTAL.fetch_add(1, Ordering::Relaxed);
    if status >= 400 {{
        ERROR_TOTAL.fetch_add(1, Ordering::Relaxed);
    }}
    trace_response(&request_id, method, route, status, started);
    maybe_shutdown(&state);
    let mut response = (StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR), body).into_response();
    let headers = response.headers_mut();
    headers.insert(axum::http::header::CONTENT_TYPE, HeaderValue::from_str(content_type).unwrap());
    headers.insert("x-corvid-request-id", HeaderValue::from_str(&request_id).unwrap());
    headers.insert(axum::http::header::CONNECTION, HeaderValue::from_static("close"));
    response
}}

fn maybe_shutdown(state: &AppState) {{
    let handled = state.handled_requests.fetch_add(1, Ordering::Relaxed) + 1;
    if matches!(state.max_requests, Some(limit) if handled >= limit) {{
        if let Some(sender) = state.shutdown.lock().unwrap().take() {{
            let _ = sender.send(());
        }}
    }}
}}

fn trace_response(request_id: &str, method: &str, route: &str, status: u16, started: Instant) {{
    eprintln!(
        "{{{{\"event\":\"corvid.server.request\",\"request_id\":{{}},\"method\":{{}},\"route\":{{}},\"status\":{{}},\"duration_ms\":{{}},\"effects\":[]}}}}",
        json_string(request_id),
        json_string(method),
        json_string(route),
        status,
        started.elapsed().as_millis()
    );
}}

struct HandlerOutput {{
    status_success: bool,
    stdout: String,
    stderr: String,
}}

enum HandlerError {{
    Spawn(String),
    TimedOut,
}}

fn run_handler(timeout: Duration) -> Result<HandlerOutput, HandlerError> {{
    if timeout.is_zero() {{
        return Err(HandlerError::TimedOut);
    }}
    let mut child = Command::new(HANDLER)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| HandlerError::Spawn(err.to_string()))?;
    let started = Instant::now();
    loop {{
        match child.try_wait() {{
            Ok(Some(status)) => {{
                let mut stdout = String::new();
                let mut stderr = String::new();
                if let Some(mut pipe) = child.stdout.take() {{
                    let _ = pipe.read_to_string(&mut stdout);
                }}
                if let Some(mut pipe) = child.stderr.take() {{
                    let _ = pipe.read_to_string(&mut stderr);
                }}
                return Ok(HandlerOutput {{
                    status_success: status.success(),
                    stdout,
                    stderr,
                }});
            }}
            Ok(None) if started.elapsed() >= timeout => {{
                let _ = child.kill();
                let _ = child.wait();
                return Err(HandlerError::TimedOut);
            }}
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(err) => return Err(HandlerError::Spawn(err.to_string())),
        }}
    }}
}}

fn handler_timeout() -> Duration {{
    let millis = std::env::var("CORVID_HANDLER_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(30_000);
    Duration::from_millis(millis)
}}

fn max_requests() -> Option<u64> {{
    std::env::var("CORVID_MAX_REQUESTS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|limit| *limit > 0)
}}

fn require_auth() -> bool {{
    std::env::var("CORVID_REQUIRE_AUTH")
        .ok()
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}}

fn rate_limit_requests() -> Option<u64> {{
    std::env::var("CORVID_RATE_LIMIT_REQUESTS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|limit| *limit > 0)
}}

fn validate_runtime_config() -> std::io::Result<()> {{
    if let Ok(port) = std::env::var("CORVID_PORT") {{
        if port.parse::<u16>().is_err() {{
            return Err(invalid_config("CORVID_PORT", "expected integer port 0-65535"));
        }}
    }}
    if let Ok(timeout) = std::env::var("CORVID_HANDLER_TIMEOUT_MS") {{
        if timeout.parse::<u64>().is_err() {{
            return Err(invalid_config("CORVID_HANDLER_TIMEOUT_MS", "expected unsigned integer milliseconds"));
        }}
    }}
    if let Ok(limit) = std::env::var("CORVID_MAX_REQUESTS") {{
        match limit.parse::<u64>() {{
            Ok(value) if value > 0 => {{}}
            _ => return Err(invalid_config("CORVID_MAX_REQUESTS", "expected positive unsigned integer")),
        }}
    }}
    if let Ok(limit) = std::env::var("CORVID_RATE_LIMIT_REQUESTS") {{
        match limit.parse::<u64>() {{
            Ok(value) if value > 0 => {{}}
            _ => return Err(invalid_config("CORVID_RATE_LIMIT_REQUESTS", "expected positive unsigned integer")),
        }}
    }}
    Ok(())
}}

fn invalid_config(name: &str, reason: &str) -> std::io::Error {{
    std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        format!("backend config {{name}} invalid: {{reason}} (value redacted)"),
    )
}}

fn request_id() -> String {{
    let counter = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("req-{{now}}-{{counter}}")
}}

fn json_string(value: &str) -> String {{
    format!("{{value:?}}")
}}
"#
    )
}

pub(super) fn render_minimal_server_source(handler_path: &Path) -> String {
    let handler = handler_path.to_string_lossy().replace('\\', "\\\\");
    format!(
        r#"use std::io::{{Read, Write}};
use std::net::{{TcpListener, TcpStream}};
use std::process::{{Command, Stdio}};
use std::sync::atomic::{{AtomicU64, Ordering}};
use std::thread;
use std::time::{{Duration, Instant, SystemTime, UNIX_EPOCH}};

const HANDLER: &str = "{handler}";
const MAX_REQUEST_BYTES: usize = 4096;
static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);
static REQUEST_TOTAL: AtomicU64 = AtomicU64::new(0);
static ERROR_TOTAL: AtomicU64 = AtomicU64::new(0);

fn main() -> std::io::Result<()> {{
    let host = std::env::var("CORVID_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = std::env::var("CORVID_PORT").unwrap_or_else(|_| "8080".to_string());
    validate_runtime_config()?;
    let listener = TcpListener::bind(format!("{{host}}:{{port}}"))?;
    let addr = listener.local_addr()?;
    println!("listening: http://{{addr}}");
    let max_requests = max_requests();
    let mut handled_requests = 0u64;
    for stream in listener.incoming() {{
        match stream {{
            Ok(stream) => {{
                let _ = handle(stream);
                handled_requests += 1;
                if matches!(max_requests, Some(limit) if handled_requests >= limit) {{
                    break;
                }}
            }}
            Err(err) => eprintln!("accept error: {{err}}"),
        }}
    }}
    Ok(())
}}

fn handle(mut stream: TcpStream) -> std::io::Result<()> {{
    let started = Instant::now();
    let request_id = request_id();
    let mut buf = [0u8; MAX_REQUEST_BYTES];
    let n = stream.read(&mut buf)?;
    if n == 0 {{
        return respond_error(
            &mut stream,
            400,
            "<unknown>",
            "<unknown>",
            "bad_request",
            "empty request",
            &request_id,
            started,
        );
    }}
    let req = String::from_utf8_lossy(&buf[..n]);
    if n == MAX_REQUEST_BYTES && !req.contains("\r\n\r\n") {{
        return respond_error(
            &mut stream,
            413,
            "<unknown>",
            "<unknown>",
            "body_too_large",
            "request exceeds server body limit",
            &request_id,
            started,
        );
    }}
    let first = req.lines().next().unwrap_or("");
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("/");
    let version = parts.next().unwrap_or("");
    if method.is_empty() || path.is_empty() || version.is_empty() {{
        return respond_error(
            &mut stream,
            400,
            "<unknown>",
            "<unknown>",
            "bad_request",
            "malformed request line",
            &request_id,
            started,
        );
    }}
    if method != "GET" {{
        return respond_error(
            &mut stream,
            405,
            method,
            path,
            "method_not_allowed",
            "method not allowed",
            &request_id,
            started,
        );
    }}
    if path == "/healthz" {{
        return respond(
            &mut stream,
            200,
            "application/json",
            "{{\"status\":\"ok\"}}",
            &request_id,
            started,
            method,
            path,
        );
    }}
    if path == "/readyz" {{
        return respond(
            &mut stream,
            200,
            "application/json",
            "{{\"ready\":true}}",
            &request_id,
            started,
            method,
            path,
        );
    }}
    if path == "/metrics" {{
        let body = format!(
            "{{{{\"request_total\":{{}},\"error_total\":{{}},\"runtime\":\"corvid-server\"}}}}",
            REQUEST_TOTAL.load(Ordering::Relaxed),
            ERROR_TOTAL.load(Ordering::Relaxed)
        );
        return respond(
            &mut stream,
            200,
            "application/json",
            &body,
            &request_id,
            started,
            method,
            path,
        );
    }}
    let output = run_handler(handler_timeout());
    match output {{
        Ok(out) if out.status_success => {{
            let body = out.stdout.trim().to_string();
            let json = format!("{{{{\"result\":{{:?}}}}}}", body);
            respond(
                &mut stream,
                200,
                "application/json",
                &json,
                &request_id,
                started,
                method,
                path,
            )
        }}
        Ok(out) => {{
            let err = out.stderr.trim().to_string();
            respond_error(
                &mut stream,
                500,
                method,
                path,
                "handler_failed",
                if err.is_empty() {{ "handler failed" }} else {{ &err }},
                &request_id,
                started,
            )
        }}
        Err(HandlerError::TimedOut) => respond_error(
            &mut stream,
            504,
            method,
            path,
            "handler_timeout",
            "handler timed out",
            &request_id,
            started,
        ),
        Err(HandlerError::Spawn(err)) => {{
            respond_error(
                &mut stream,
                500,
                method,
                path,
                "handler_spawn_failed",
                &err,
                &request_id,
                started,
            )
        }}
    }}
}}

fn respond_error(
    stream: &mut TcpStream,
    status: u16,
    method: &str,
    route: &str,
    kind: &str,
    message: &str,
    request_id: &str,
    started: Instant,
) -> std::io::Result<()> {{
    let body = format!(
        "{{{{\"request_id\":{{}},\"route\":{{}},\"kind\":{{}},\"message\":{{}},\"duration_ms\":{{}}}}}}",
        json_string(request_id),
        json_string(route),
        json_string(kind),
        json_string(message),
        started.elapsed().as_millis()
    );
    write_response(
        stream,
        status,
        "application/json",
        &body,
        &request_id,
        started,
        method,
        route,
    )
}}

fn respond(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &str,
    request_id: &str,
    started: Instant,
    method: &str,
    route: &str,
) -> std::io::Result<()> {{
    write_response(stream, status, content_type, body, &request_id, started, method, route)
}}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &str,
    request_id: &str,
    started: Instant,
    method: &str,
    route: &str,
) -> std::io::Result<()> {{
    let reason = match status {{
        200 => "OK",
        400 => "Bad Request",
        413 => "Payload Too Large",
        405 => "Method Not Allowed",
        504 => "Gateway Timeout",
        _ => "Internal Server Error",
    }};
    let response = format!(
        "HTTP/1.1 {{status}} {{reason}}\r\ncontent-type: {{content_type}}\r\ncontent-length: {{}}\r\nx-corvid-request-id: {{request_id}}\r\nconnection: close\r\n\r\n{{body}}",
        body.as_bytes().len()
    );
    REQUEST_TOTAL.fetch_add(1, Ordering::Relaxed);
    if status >= 400 {{
        ERROR_TOTAL.fetch_add(1, Ordering::Relaxed);
    }}
    trace_response(request_id, method, route, status, started);
    stream.write_all(response.as_bytes())
}}

fn trace_response(request_id: &str, method: &str, route: &str, status: u16, started: Instant) {{
    eprintln!(
        "{{{{\"event\":\"corvid.server.request\",\"request_id\":{{}},\"method\":{{}},\"route\":{{}},\"status\":{{}},\"duration_ms\":{{}},\"effects\":[]}}}}",
        json_string(request_id),
        json_string(method),
        json_string(route),
        status,
        started.elapsed().as_millis()
    );
}}

struct HandlerOutput {{
    status_success: bool,
    stdout: String,
    stderr: String,
}}

enum HandlerError {{
    Spawn(String),
    TimedOut,
}}

fn run_handler(timeout: Duration) -> Result<HandlerOutput, HandlerError> {{
    if timeout.is_zero() {{
        return Err(HandlerError::TimedOut);
    }}
    let mut child = Command::new(HANDLER)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| HandlerError::Spawn(err.to_string()))?;
    let started = Instant::now();
    loop {{
        match child.try_wait() {{
            Ok(Some(status)) => {{
                let mut stdout = String::new();
                let mut stderr = String::new();
                if let Some(mut pipe) = child.stdout.take() {{
                    let _ = pipe.read_to_string(&mut stdout);
                }}
                if let Some(mut pipe) = child.stderr.take() {{
                    let _ = pipe.read_to_string(&mut stderr);
                }}
                return Ok(HandlerOutput {{
                    status_success: status.success(),
                    stdout,
                    stderr,
                }});
            }}
            Ok(None) if started.elapsed() >= timeout => {{
                let _ = child.kill();
                let _ = child.wait();
                return Err(HandlerError::TimedOut);
            }}
            Ok(None) => thread::sleep(Duration::from_millis(5)),
            Err(err) => return Err(HandlerError::Spawn(err.to_string())),
        }}
    }}
}}

fn handler_timeout() -> Duration {{
    let millis = std::env::var("CORVID_HANDLER_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(30_000);
    Duration::from_millis(millis)
}}

fn max_requests() -> Option<u64> {{
    std::env::var("CORVID_MAX_REQUESTS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|limit| *limit > 0)
}}

fn validate_runtime_config() -> std::io::Result<()> {{
    if let Ok(port) = std::env::var("CORVID_PORT") {{
        if port.parse::<u16>().is_err() {{
            return Err(invalid_config("CORVID_PORT", "expected integer port 0-65535"));
        }}
    }}
    if let Ok(timeout) = std::env::var("CORVID_HANDLER_TIMEOUT_MS") {{
        if timeout.parse::<u64>().is_err() {{
            return Err(invalid_config("CORVID_HANDLER_TIMEOUT_MS", "expected unsigned integer milliseconds"));
        }}
    }}
    if let Ok(limit) = std::env::var("CORVID_MAX_REQUESTS") {{
        match limit.parse::<u64>() {{
            Ok(value) if value > 0 => {{}}
            _ => return Err(invalid_config("CORVID_MAX_REQUESTS", "expected positive unsigned integer")),
        }}
    }}
    Ok(())
}}

fn invalid_config(name: &str, reason: &str) -> std::io::Error {{
    std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        format!("backend config {{name}} invalid: {{reason}} (value redacted)"),
    )
}}

fn request_id() -> String {{
    let counter = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("req-{{now}}-{{counter}}")
}}

fn json_string(value: &str) -> String {{
    format!("{{value:?}}")
}}
"#
    )
}
