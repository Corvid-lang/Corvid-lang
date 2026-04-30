//! `corvid doctor` CLI dispatch — slice 24 / developer-experience
//! diagnostics, decomposed in Phase 20j-A1.
//!
//! Two entry points:
//!
//! - [`cmd_doctor`] runs the original Phase-20 readiness sweep
//!   (cargo + rustc + targets + provider connectivity smoke tests).
//! - [`cmd_doctor_v2`] is the slice 24 expansion that adds
//!   per-environment-variable validators, `Corvid.lock` discovery,
//!   and a Python-runtime probe.
//!
//! All process-spawn + env-validation + path-walk helpers
//! (`command_succeeds`, `command_output`, `check_u16_env`,
//! `check_u64_env`, `check_positive_u64_env`,
//! `check_hex_key_env`, `find_upward`) are private to this
//! module — no other CLI command consumes them.

use anyhow::Result;
use std::path::{Path, PathBuf};

#[allow(dead_code)]
pub(crate) fn cmd_doctor() -> Result<u8> {
    use corvid_driver::load_dotenv_walking;

    println!("corvid doctor — checking local environment...\n");

    // Try loading .env first so the rest of the checks see what programs would.
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match load_dotenv_walking(&cwd) {
        Some(p) => println!("  ✓ .env loaded from {}", p.display()),
        None => println!("  · no .env file found from cwd upward (optional)"),
    }

    // CORVID_MODEL
    let model = std::env::var("CORVID_MODEL").ok();
    match &model {
        Some(v) => println!("  ✓ CORVID_MODEL = {v}"),
        None => println!(
            "  · CORVID_MODEL not set. Set one (e.g. `export CORVID_MODEL=gpt-4o-mini` or\n    `claude-opus-4-6`) or put `default_model = \"...\"` in corvid.toml under [llm]."
        ),
    }

    // Anthropic
    if std::env::var("ANTHROPIC_API_KEY").is_ok() {
        println!("  ✓ ANTHROPIC_API_KEY set (Claude models available)");
    } else {
        println!("  · ANTHROPIC_API_KEY not set — Claude calls will error at the prompt site");
    }

    // OpenAI
    if std::env::var("OPENAI_API_KEY").is_ok() {
        println!("  ✓ OPENAI_API_KEY set (GPT / o-series models available)");
    } else {
        println!("  · OPENAI_API_KEY not set — OpenAI calls will error at the prompt site");
    }

    // Cross-check: model prefix vs. available keys.
    if let Some(m) = &model {
        if m.starts_with("claude-") && std::env::var("ANTHROPIC_API_KEY").is_err() {
            println!("  ✗ CORVID_MODEL is `{m}` but ANTHROPIC_API_KEY is not set");
        }
        let openai_prefixes = ["gpt-", "o1-", "o3-", "o4-"];
        if openai_prefixes.iter().any(|p| m.starts_with(p))
            && std::env::var("OPENAI_API_KEY").is_err()
        {
            println!("  ✗ CORVID_MODEL is `{m}` but OPENAI_API_KEY is not set");
        }
    }

    // Python (legacy `--target=python` users only)
    let has_python = std::process::Command::new("python3")
        .arg("--version")
        .output()
        .ok()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if has_python {
        println!("  · python3 detected (legacy `--target=python` available)");
    } else {
        println!("  · python3 not detected (only needed for `--target=python`)");
    }

    println!();
    println!("native `corvid run` works without Python. Configure CORVID_MODEL + the");
    println!("matching API key and prompt-only programs run end-to-end.");
    Ok(0)
}

pub(crate) fn cmd_doctor_v2() -> Result<u8> {
    use corvid_driver::load_dotenv_walking;

    println!("corvid doctor - checking local environment...\n");
    let mut ok = true;
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    match load_dotenv_walking(&cwd) {
        Some(p) => println!("  OK .env loaded from {}", p.display()),
        None => println!("  .. no .env file found from cwd upward (optional)"),
    }

    if command_succeeds("cargo", &["--version"]) {
        println!("  OK cargo detected");
    } else {
        ok = false;
        println!("  XX cargo not found in PATH");
    }

    if command_succeeds("rustc", &["--version"]) {
        println!("  OK rustc detected");
    } else {
        ok = false;
        println!("  XX rustc not found in PATH");
    }

    if command_output("rustup", &["target", "list", "--installed"])
        .map(|stdout| stdout.contains("wasm32-unknown-unknown"))
        .unwrap_or(false)
    {
        println!("  OK wasm32-unknown-unknown target installed");
    } else {
        println!("  .. wasm32-unknown-unknown target missing (`rustup target add wasm32-unknown-unknown`)");
    }

    let model = std::env::var("CORVID_MODEL").ok();
    match &model {
        Some(v) => println!("  OK CORVID_MODEL = {v}"),
        None => println!("  .. CORVID_MODEL not set"),
    }

    let anthropic = std::env::var("ANTHROPIC_API_KEY").is_ok();
    let openai = std::env::var("OPENAI_API_KEY").is_ok();
    println!(
        "  {} ANTHROPIC_API_KEY {}",
        if anthropic { "OK" } else { ".." },
        if anthropic { "set" } else { "not set" }
    );
    println!(
        "  {} OPENAI_API_KEY {}",
        if openai { "OK" } else { ".." },
        if openai { "set" } else { "not set" }
    );

    if let Some(m) = &model {
        if m.starts_with("claude-") && !anthropic {
            ok = false;
            println!("  XX CORVID_MODEL is `{m}` but ANTHROPIC_API_KEY is not set");
        }
        if ["gpt-", "o1-", "o3-", "o4-"]
            .iter()
            .any(|prefix| m.starts_with(prefix))
            && !openai
        {
            ok = false;
            println!("  XX CORVID_MODEL is `{m}` but OPENAI_API_KEY is not set");
        }
    }

    println!(
        "  {} ollama {}",
        if command_succeeds("ollama", &["--version"]) {
            "OK"
        } else {
            ".."
        },
        if command_succeeds("ollama", &["--version"]) {
            "detected"
        } else {
            "not detected (only needed for local-model demos)"
        }
    );

    let trace_dir = cwd.join("target").join("trace");
    if trace_dir.exists() {
        println!("  OK replay storage present at {}", trace_dir.display());
    } else {
        println!(
            "  .. replay storage not initialized yet (expected at {})",
            trace_dir.display()
        );
    }

    match std::env::var("CORVID_APPROVER").ok() {
        Some(value) => println!("  OK CORVID_APPROVER = {value}"),
        None => println!("  .. CORVID_APPROVER not set"),
    }

    if !check_u16_env("CORVID_PORT", "backend listen port") {
        ok = false;
    }
    if !check_u64_env("CORVID_HANDLER_TIMEOUT_MS", "backend handler timeout") {
        ok = false;
    }
    if !check_positive_u64_env(
        "CORVID_MAX_REQUESTS",
        "backend graceful drain request limit",
    ) {
        ok = false;
    }
    if !check_hex_key_env("CORVID_TOKEN_KEY", 64, "connector token encryption key") {
        ok = false;
    }

    match find_upward(&cwd, "Corvid.lock") {
        Some(path) => println!("  OK registry lockfile found at {}", path.display()),
        None => println!("  .. no Corvid.lock found from cwd upward"),
    }

    let has_python =
        command_succeeds("python3", &["--version"]) || command_succeeds("python", &["--version"]);
    println!(
        "  .. {}",
        if has_python {
            "python detected (legacy --target=python available)"
        } else {
            "python not detected (only needed for --target=python)"
        }
    );

    println!();
    println!("native `corvid run` works without Python. Configure CORVID_MODEL + the matching API key and prompt-only programs run end-to-end.");
    Ok(if ok { 0 } else { 1 })
}

fn command_succeeds(program: &str, args: &[&str]) -> bool {
    std::process::Command::new(program)
        .args(args)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn command_output(program: &str, args: &[&str]) -> Option<String> {
    std::process::Command::new(program)
        .args(args)
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                Some(String::from_utf8_lossy(&output.stdout).to_string())
            } else {
                None
            }
        })
}



fn check_u16_env(name: &str, label: &str) -> bool {
    match std::env::var(name) {
        Ok(value) if value.parse::<u16>().is_ok() => {
            println!("  OK {name} valid ({label})");
            true
        }
        Ok(_) => {
            println!("  XX {name} invalid ({label}); value redacted");
            false
        }
        Err(_) => {
            println!("  .. {name} not set ({label})");
            true
        }
    }
}

fn check_u64_env(name: &str, label: &str) -> bool {
    match std::env::var(name) {
        Ok(value) if value.parse::<u64>().is_ok() => {
            println!("  OK {name} valid ({label})");
            true
        }
        Ok(_) => {
            println!("  XX {name} invalid ({label}); value redacted");
            false
        }
        Err(_) => {
            println!("  .. {name} not set ({label})");
            true
        }
    }
}

fn check_positive_u64_env(name: &str, label: &str) -> bool {
    match std::env::var(name) {
        Ok(value) => match value.parse::<u64>() {
            Ok(parsed) if parsed > 0 => {
                println!("  OK {name} valid ({label})");
                true
            }
            _ => {
                println!("  XX {name} invalid ({label}); value redacted");
                false
            }
        },
        Err(_) => {
            println!("  .. {name} not set ({label})");
            true
        }
    }
}

fn check_hex_key_env(name: &str, expected_len: usize, label: &str) -> bool {
    match std::env::var(name) {
        Ok(value)
            if value.len() == expected_len && value.chars().all(|ch| ch.is_ascii_hexdigit()) =>
        {
            println!("  OK {name} valid ({label})");
            true
        }
        Ok(_) => {
            println!("  XX {name} invalid ({label}); value redacted");
            false
        }
        Err(_) => {
            println!("  .. {name} not set ({label})");
            true
        }
    }
}

fn find_upward(start: &Path, name: &str) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(dir) = current {
        let candidate = dir.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
        current = dir.parent();
    }
    None
}
