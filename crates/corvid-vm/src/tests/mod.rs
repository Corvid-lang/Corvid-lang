
use super::*;
use corvid_ast::{BackpressurePolicy, Span};
use corvid_ir::lower;
use corvid_resolve::resolve;
use corvid_runtime::{
    llm::{mock::MockAdapter, TokenUsage},
    ApprovalDecision, ProgrammaticApprover, RegisteredModel, Runtime, RuntimeError, TraceEvent,
};
use corvid_syntax::{lex, parse_file};
use corvid_types::typecheck;
use serde_json::json;
use std::sync::Arc;

mod stream;
mod core;
mod dispatch;

/// Compile source text all the way down to IR. Panics on any frontend
/// error — tests should pass clean programs.
fn ir_of(src: &str) -> corvid_ir::IrFile {
    let tokens = lex(src).expect("lex");
    let (file, perr) = parse_file(&tokens);
    assert!(perr.is_empty(), "parse: {perr:?}");
    let resolved = resolve(&file);
    assert!(resolved.errors.is_empty(), "resolve: {:?}", resolved.errors);
    let checked = typecheck(&file, &resolved);
    assert!(checked.errors.is_empty(), "typecheck: {:?}", checked.errors);
    lower(&file, &resolved, &checked)
}

/// A runtime with no tools, no LLMs, and an always-yes approver.
/// Suitable for tests that only exercise pure computation.
fn empty_runtime() -> Runtime {
    Runtime::builder()
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .build()
}

async fn collect_stream(value: Value) -> Result<Vec<Value>, InterpError> {
    let Value::Stream(stream) = value else {
        panic!("expected stream value");
    };
    let mut out = Vec::new();
    while let Some(item) = stream.next().await {
        out.push(item?);
    }
    Ok(out)
}
