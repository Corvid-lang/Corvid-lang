//! Mock `#[tool]` implementations used by the Phase 14 parity harness.
//!
//! Every tool here reads its return value from a dedicated env var so
//! the harness can reuse one linked staticlib across fixtures that need
//! different values. Setting env vars per-invocation avoids a rebuild
//! per test — Corvid programs are the only things that change between
//! fixtures, not the Rust tool implementations.
//!
//! The env-var indirection is NOT the mechanism real tools should use
//! at runtime. Real tools compute their return values (e.g. an LLM call,
//! a DB query). These mocks exist purely to keep tests fast + isolated
//! while Phase 14's typed-ABI dispatch path is exercised end-to-end.

use corvid_macros::tool;

fn env_i64(key: &str) -> i64 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| {
            panic!(
                "corvid-test-tools: env var `{key}` is missing or not a valid i64; \
                 set it before spawning the test binary"
            )
        })
}

fn env_bool(key: &str) -> bool {
    match std::env::var(key).ok().as_deref() {
        Some("1") | Some("true") | Some("True") => true,
        Some("0") | Some("false") | Some("False") => false,
        other => panic!(
            "corvid-test-tools: env var `{key}` = {other:?} is not a recognised bool"
        ),
    }
}

fn env_f64(key: &str) -> f64 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| {
            panic!(
                "corvid-test-tools: env var `{key}` is missing or not a valid f64"
            )
        })
}

fn env_string(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| {
        panic!("corvid-test-tools: env var `{key}` is missing")
    })
}

// ------------------------------------------------------------
// Phase 13-compatible mock tools.
// Kept under their original names so the existing parity fixtures
// keep working after the dispatch mechanism flip from env-var-registry
// to typed-ABI-direct-call.
// ------------------------------------------------------------

#[tool("answer")]
async fn answer() -> i64 {
    env_i64("CORVID_TEST_TOOL_ANSWER")
}

#[tool("base")]
async fn base() -> i64 {
    env_i64("CORVID_TEST_TOOL_BASE")
}

#[tool("flag")]
async fn flag() -> i64 {
    env_i64("CORVID_TEST_TOOL_FLAG")
}

#[tool("a")]
async fn a() -> i64 {
    env_i64("CORVID_TEST_TOOL_A")
}

#[tool("b")]
async fn b() -> i64 {
    env_i64("CORVID_TEST_TOOL_B")
}

#[tool("leaf")]
async fn leaf() -> i64 {
    env_i64("CORVID_TEST_TOOL_LEAF")
}

// ------------------------------------------------------------
// Phase 14 mock tools exercising each scalar arg type + mixed shapes.
// ------------------------------------------------------------

#[tool("double_int")]
async fn double_int(n: i64) -> i64 {
    n * 2
}

#[tool("negate_bool")]
async fn negate_bool(b: bool) -> bool {
    !b
}

#[tool("triple_float")]
async fn triple_float(x: f64) -> f64 {
    x * 3.0
}

#[tool("echo_string")]
async fn echo_string(s: String) -> String {
    s
}

#[tool("greet_string")]
async fn greet_string(name: String) -> String {
    format!("hi {name}")
}

#[tool("string_len")]
async fn string_len(s: String) -> i64 {
    s.chars().count() as i64
}

#[tool("add_two")]
async fn add_two(a: i64, b: i64) -> i64 {
    a + b
}
