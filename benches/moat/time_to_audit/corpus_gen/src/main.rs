//! Generates the `time_to_audit` benchmark corpus by driving the
//! refund_bot agent against a fixed list of varied tickets, each
//! paired with a varied mocked LLM decision and approver outcome.
//!
//! The agent under test is `examples/refund_bot_demo/src/refund_bot.cor`.
//! Each invocation produces one JSONL trace under
//! `<--out>/run-<NN>.jsonl`. The output filenames are sequence-numbered
//! (not timestamp-keyed) so the committed corpus is reproducible: a
//! second run of corpus_gen overwrites the same files with traces
//! that differ only in the normalized fields (ts_ms, run_id,
//! token_id, *_at_ms).
//!
//! Usage from the workspace root:
//!
//! ```sh
//! cargo run -q -p refund_bot_corpus_gen -- \
//!     --out benches/moat/time_to_audit/corpus/corvid
//! ```

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use corvid_driver::{
    build_struct, compile_to_ir, run_with_runtime, MockAdapter, ProgrammaticApprover,
    Runtime, Value,
};
use serde_json::{json, Value as JsonValue};

const SOURCE_REL: &str = "examples/refund_bot_demo/src/refund_bot.cor";

#[derive(Parser, Debug)]
#[command(about = "Generate the time_to_audit corpus.", long_about = None)]
struct Args {
    /// Directory to write `run-<NN>.jsonl` files into.
    #[arg(long)]
    out: PathBuf,
}

#[derive(Clone)]
struct CorpusEntry {
    seq: usize,
    order_id: &'static str,
    user_id: &'static str,
    amount: f64,
    message: &'static str,
    should_refund: bool,
    reason: &'static str,
    approve: bool,
}

fn corpus() -> Vec<CorpusEntry> {
    vec![
        CorpusEntry {
            seq: 1,
            order_id: "ord_42",
            user_id: "user_1",
            amount: 49.99,
            message: "package arrived broken — please refund",
            should_refund: true,
            reason: "user reported legitimate complaint",
            approve: true,
        },
        CorpusEntry {
            seq: 2,
            order_id: "ord_77",
            user_id: "user_2",
            amount: 129.50,
            message: "wrong size, returning unopened",
            should_refund: true,
            reason: "size mismatch with order record",
            approve: true,
        },
        CorpusEntry {
            seq: 3,
            order_id: "ord_19",
            user_id: "user_1",
            amount: 14.95,
            message: "looks ok, just changed my mind",
            should_refund: false,
            reason: "no defect; outside return window",
            approve: false,
        },
        CorpusEntry {
            seq: 4,
            order_id: "ord_88",
            user_id: "user_3",
            amount: 219.00,
            message: "delivered to wrong address; never received",
            should_refund: true,
            reason: "non-delivery confirmed by carrier note",
            approve: true,
        },
        CorpusEntry {
            seq: 5,
            order_id: "ord_5",
            user_id: "user_2",
            amount: 8.40,
            message: "trial item, not what was advertised",
            should_refund: false,
            reason: "marketing copy matches product as shipped",
            approve: false,
        },
        CorpusEntry {
            seq: 6,
            order_id: "ord_91",
            user_id: "user_2",
            amount: 67.25,
            message: "received only half the order",
            should_refund: true,
            reason: "split-shipment evidence in ticket",
            approve: true,
        },
    ]
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let source_path = locate_source(SOURCE_REL)?;
    let source = std::fs::read_to_string(&source_path)?;

    std::fs::create_dir_all(&args.out)?;

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

    for entry in corpus() {
        let entry_dir = args.out.join(format!("run-{:02}", entry.seq));
        // Each run gets its own trace dir so the runtime emits exactly
        // one JSONL file per entry; we rename it to a stable sequence
        // name afterwards.
        let _ = std::fs::remove_dir_all(&entry_dir);
        std::fs::create_dir_all(&entry_dir)?;

        let llm_reply = json!({
            "should_refund": entry.should_refund,
            "reason": entry.reason,
        });
        let order_value: JsonValue = json!({
            "id": entry.order_id,
            "amount": entry.amount,
            "user_id": entry.user_id,
        });
        let order_value_clone = order_value.clone();
        let amount = entry.amount;
        let approve = entry.approve;

        let runtime = Runtime::builder()
            .tool("get_order", move |_args| {
                let v = order_value_clone.clone();
                async move { Ok(v) }
            })
            .tool("issue_refund", move |args| async move {
                let id = args.first().and_then(|v| v.as_str()).unwrap_or("");
                Ok(json!({
                    "refund_id": format!("rf_{id}"),
                    "amount": amount,
                }))
            })
            .approver(Arc::new(if approve {
                ProgrammaticApprover::always_yes()
            } else {
                ProgrammaticApprover::always_no()
            }))
            .llm(Arc::new(
                MockAdapter::new("mock-1").reply("decide_refund", llm_reply),
            ))
            .default_model("mock-1")
            .trace_to(&entry_dir)
            .build();

        let ticket = build_struct(
            ticket_id,
            "Ticket",
            [
                (
                    "order_id".to_string(),
                    Value::String(Arc::from(entry.order_id)),
                ),
                (
                    "user_id".to_string(),
                    Value::String(Arc::from(entry.user_id)),
                ),
                (
                    "message".to_string(),
                    Value::String(Arc::from(entry.message)),
                ),
            ],
        );

        // Run; ignore the agent's typed result — we want the trace.
        let _ = run_with_runtime(
            &source_path,
            Some("refund_bot"),
            vec![ticket],
            &runtime,
        )
        .await;

        // Move the (single) trace file out of its run-NN/ subdir to a
        // stable sequence-numbered filename at the corpus root.
        let mut produced: Vec<PathBuf> = std::fs::read_dir(&entry_dir)?
            .flatten()
            .map(|d| d.path())
            .filter(|p| p.is_file())
            .collect();
        if produced.is_empty() {
            anyhow::bail!(
                "run #{} produced no trace file under {}",
                entry.seq,
                entry_dir.display()
            );
        }
        if produced.len() > 1 {
            anyhow::bail!(
                "run #{} produced {} trace files under {}; expected 1",
                entry.seq,
                produced.len(),
                entry_dir.display()
            );
        }
        let src = produced.pop().unwrap();
        let dst = args.out.join(format!("run-{:02}.jsonl", entry.seq));
        std::fs::rename(&src, &dst)?;
        std::fs::remove_dir_all(&entry_dir)?;
        println!(
            "wrote {} ({} ticket={} approve={})",
            dst.display(),
            entry.seq,
            entry.order_id,
            entry.approve
        );
    }

    Ok(())
}

fn locate_source(rel: &str) -> anyhow::Result<PathBuf> {
    let direct = PathBuf::from(rel);
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
