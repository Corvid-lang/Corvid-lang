use corvid_driver::compile_to_ir_with_config_at_path;
use std::fs;

#[test]
fn std_ai_compiles_as_corvid_source() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repo root");
    let source_path = repo.join("std").join("ai.cor");
    let source = fs::read_to_string(&source_path).expect("std/ai.cor");

    compile_to_ir_with_config_at_path(&source, &source_path, None)
        .expect("std.ai should compile as a standalone Corvid module");
}

#[test]
fn std_ai_imported_helpers_typecheck() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("std")).unwrap();
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repo root");
    fs::copy(repo.join("std").join("ai.cor"), dir.path().join("std").join("ai.cor")).unwrap();

    let main_path = dir.path().join("main.cor");
    let source = r#"
import "./std/ai" use AiMessage, AiSession, user_message, start_session, next_turn, tool_ok, confidence, render_prompt_pair, render_message, rendered_prompt

agent main() -> String:
    msg = user_message("hello")
    sess = start_session("s1", "demo")
    next = next_turn(sess)
    envelope = tool_ok("lookup", msg.content)
    conf = confidence(0.8, 0.5)
    prompt_line = render_prompt_pair("query", msg.content)
    rendered = rendered_prompt("search", render_message(msg))
    if conf.accepted:
        return envelope.value + " " + prompt_line + " " + rendered.template_name
    else:
        return next.title
"#;
    fs::write(&main_path, source).unwrap();

    compile_to_ir_with_config_at_path(source, &main_path, None)
        .expect("program importing std.ai helpers should compile");
}

#[test]
fn std_http_compiles_as_corvid_source() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repo root");
    let source_path = repo.join("std").join("http.cor");
    let source = fs::read_to_string(&source_path).expect("std/http.cor");

    compile_to_ir_with_config_at_path(&source, &source_path, None)
        .expect("std.http should compile as a standalone Corvid module");
}

#[test]
fn std_io_compiles_as_corvid_source() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repo root");
    let source_path = repo.join("std").join("io.cor");
    let source = fs::read_to_string(&source_path).expect("std/io.cor");

    compile_to_ir_with_config_at_path(&source, &source_path, None)
        .expect("std.io should compile as a standalone Corvid module");
}

#[test]
fn std_secrets_compiles_as_corvid_source() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repo root");
    let source_path = repo.join("std").join("secrets.cor");
    let source = fs::read_to_string(&source_path).expect("std/secrets.cor");

    compile_to_ir_with_config_at_path(&source, &source_path, None)
        .expect("std.secrets should compile as a standalone Corvid module");
}

#[test]
fn std_observe_compiles_as_corvid_source() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repo root");
    let source_path = repo.join("std").join("observe.cor");
    let source = fs::read_to_string(&source_path).expect("std/observe.cor");

    compile_to_ir_with_config_at_path(&source, &source_path, None)
        .expect("std.observe should compile as a standalone Corvid module");
}

#[test]
fn std_cache_compiles_as_corvid_source() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repo root");
    let source_path = repo.join("std").join("cache.cor");
    let source = fs::read_to_string(&source_path).expect("std/cache.cor");

    compile_to_ir_with_config_at_path(&source, &source_path, None)
        .expect("std.cache should compile as a standalone Corvid module");
}

#[test]
fn std_queue_compiles_as_corvid_source() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repo root");
    let source_path = repo.join("std").join("queue.cor");
    let source = fs::read_to_string(&source_path).expect("std/queue.cor");

    compile_to_ir_with_config_at_path(&source, &source_path, None)
        .expect("std.queue should compile as a standalone Corvid module");
}

#[test]
fn std_agent_compiles_as_corvid_source() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repo root");
    let source_path = repo.join("std").join("agent.cor");
    let source = fs::read_to_string(&source_path).expect("std/agent.cor");

    compile_to_ir_with_config_at_path(&source, &source_path, None)
        .expect("std.agent should compile as a standalone Corvid module");
}

#[test]
fn std_rag_compiles_as_corvid_source() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repo root");
    let source_path = repo.join("std").join("rag.cor");
    let source = fs::read_to_string(&source_path).expect("std/rag.cor");

    compile_to_ir_with_config_at_path(&source, &source_path, None)
        .expect("std.rag should compile as a standalone Corvid module");
}

#[test]
fn std_effects_compiles_as_corvid_source() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repo root");
    let source_path = repo.join("std").join("effects.cor");
    let source = fs::read_to_string(&source_path).expect("std/effects.cor");

    compile_to_ir_with_config_at_path(&source, &source_path, None)
        .expect("std.effects should compile as a standalone Corvid module");
}
