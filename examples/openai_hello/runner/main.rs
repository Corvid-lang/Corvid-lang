//! Minimal OpenAI demo. Calls a real `gpt-4o-mini` (or whatever
//! `CORVID_MODEL` is set to) and returns a typed `Greeting` struct.
//!
//! Usage:
//!
//! ```sh
//! export OPENAI_API_KEY=sk-...
//! export CORVID_MODEL=gpt-4o-mini
//! cargo run -p openai_hello
//! ```
//!
//! Or put the same vars in `.env` at the repo root.

use std::sync::Arc;

use corvid_driver::{
    fresh_run_id, load_dotenv_walking, OpenAiAdapter, RedactionSet, Runtime, Tracer, Value,
};

const SOURCE_REL: &str = "examples/openai_hello/src/hello.cor";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let _ = load_dotenv_walking(&cwd);

    let api_key = std::env::var("OPENAI_API_KEY").map_err(|_| {
        anyhow::anyhow!("set OPENAI_API_KEY in your environment or in .env")
    })?;
    let model = std::env::var("CORVID_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into());

    let source_path = locate_source(SOURCE_REL)?;
    let trace_dir = source_path
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join("target").join("trace"))
        .unwrap_or_else(|| std::path::PathBuf::from("target/trace"));

    let tracer = Tracer::open(&trace_dir, fresh_run_id())
        .with_redaction(RedactionSet::from_env());

    let runtime = Runtime::builder()
        .llm(Arc::new(OpenAiAdapter::new(api_key)))
        .default_model(&model)
        .tracer(tracer)
        .build();

    let result = corvid_driver::run_with_runtime(
        &source_path,
        Some("greet"),
        vec![Value::String(Arc::from("Disan"))],
        &runtime,
    )
    .await?;

    print_greeting(&result);
    println!("model used: {model}");
    println!("trace written under {}", trace_dir.display());
    Ok(())
}

fn print_greeting(v: &Value) {
    match v {
        Value::Struct(s) => {
            let salutation = field_str(s, "salutation");
            let target = field_str(s, "target");
            println!("greeting: {salutation}, {target}!");
        }
        other => println!("returned: {other}"),
    }
}

fn field_str(s: &corvid_driver::StructValue, key: &str) -> String {
    s.get_field(key)
        .as_ref()
        .map(|v| match v {
            Value::String(s) => s.to_string(),
            other => other.to_string(),
        })
        .unwrap_or_else(|| "<missing>".into())
}

fn locate_source(rel: &str) -> anyhow::Result<std::path::PathBuf> {
    let direct = std::path::PathBuf::from(rel);
    if direct.exists() {
        return Ok(direct);
    }
    let mut here = std::env::current_dir()?;
    loop {
        let candidate = here.join(rel);
        if candidate.exists() {
            return Ok(candidate);
        }
        if !here.pop() {
            anyhow::bail!("could not locate `{rel}` from any ancestor of the cwd");
        }
    }
}
