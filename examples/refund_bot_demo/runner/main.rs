//! Native runner for the refund_bot demo.
//!
//! Wires mock tool implementations and a mock LLM adapter into a Corvid
//! `Runtime`, then runs the agent end-to-end. No Python on the path.
//!
//! Usage from the workspace root:
//!
//! ```sh
//! cargo run -p refund_bot_demo
//! ```

use std::sync::Arc;

use corvid_driver::{
    build_struct, compile_to_ir, run_with_runtime, MockAdapter, ProgrammaticApprover,
    Runtime, Value,
};
use serde_json::json;

const SOURCE_REL: &str = "examples/refund_bot_demo/src/refund_bot.cor";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Locate the .cor source relative to the workspace root.
    let source_path = locate_source(SOURCE_REL)?;
    let source = std::fs::read_to_string(&source_path)?;

    let trace_dir = source_path
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join("target").join("trace"))
        .unwrap_or_else(|| std::path::PathBuf::from("target/trace"));

    let runtime = Runtime::builder()
        .tool("get_order", |args| async move {
            let id = args.first().and_then(|v| v.as_str()).unwrap_or("");
            Ok(json!({
                "id": id,
                "amount": 49.99,
                "user_id": "user_1",
            }))
        })
        .tool("issue_refund", |args| async move {
            let id = args.first().and_then(|v| v.as_str()).unwrap_or("");
            let amount = args.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0);
            Ok(json!({
                "refund_id": format!("rf_{id}"),
                "amount": amount,
            }))
        })
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .llm(Arc::new(MockAdapter::new("mock-1").reply(
            "decide_refund",
            json!({
                "should_refund": true,
                "reason": "user reported legitimate complaint",
            }),
        )))
        .default_model("mock-1")
        .trace_to(&trace_dir)
        .build();

    // Construct a Ticket struct using the IR's resolved DefId for `Ticket`.
    let ir = compile_to_ir(&source).map_err(|diags| {
        anyhow::anyhow!(
            "compile failed with {} diagnostic(s); first: {}",
            diags.len(),
            diags
                .first()
                .map(|d| d.message.clone())
                .unwrap_or_default()
        )
    })?;
    let ticket_id = ir
        .types
        .iter()
        .find(|t| t.name == "Ticket")
        .ok_or_else(|| anyhow::anyhow!("expected `Ticket` type in source"))?
        .id;
    let ticket = build_struct(
        ticket_id,
        "Ticket",
        [
            ("order_id".to_string(), Value::String(std::sync::Arc::from("ord_42"))),
            ("user_id".to_string(), Value::String(std::sync::Arc::from("user_1"))),
            (
                "message".to_string(),
                Value::String(std::sync::Arc::from(
                    "package arrived broken — please refund",
                )),
            ),
        ],
    );

    let result = run_with_runtime(&source_path, Some("refund_bot"), vec![ticket], &runtime).await?;

    print_decision(&result);
    println!("trace written under {}", trace_dir.display());
    Ok(())
}

fn print_decision(v: &Value) {
    match v {
        Value::Struct(s) => {
            let should_refund = s
                .fields
                .get("should_refund")
                .and_then(|v| match v {
                    Value::Bool(b) => Some(*b),
                    _ => None,
                })
                .unwrap_or(false);
            let reason = s
                .fields
                .get("reason")
                .map(|v| v.to_string())
                .unwrap_or_else(|| "<no reason>".into());
            println!("refund_bot decided: should_refund={should_refund} reason={reason}");
        }
        other => println!("refund_bot returned: {other}"),
    }
}

fn locate_source(rel: &str) -> anyhow::Result<std::path::PathBuf> {
    // Try CWD first; fall back to walking parent directories until a
    // matching file appears (handles `cargo run` from various cwds).
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
