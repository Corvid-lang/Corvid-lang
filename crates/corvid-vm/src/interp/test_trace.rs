use corvid_ast::BinaryOp;
use corvid_ir::IrEvalAssert;
use corvid_trace_schema::{read_events_from_path, validate_supported_schema, TraceEvent};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub(super) struct TraceFixture {
    pub path: PathBuf,
    pub events: Vec<TraceEvent>,
}

pub(super) fn load_trace_fixture(spec: &str, root: &Path) -> Result<TraceFixture, String> {
    let path = resolve_trace_path(spec, root);
    let events = read_events_from_path(&path)
        .map_err(|error| format!("failed to read trace fixture `{}`: {error}", path.display()))?;
    validate_supported_schema(&events)
        .map_err(|error| format!("unsupported trace fixture `{}`: {error}", path.display()))?;
    if !events
        .iter()
        .any(|event| matches!(event, TraceEvent::RunStarted { .. }))
    {
        return Err(format!(
            "trace fixture `{}` has no run_started event",
            path.display()
        ));
    }
    if !events
        .iter()
        .any(|event| matches!(event, TraceEvent::RunCompleted { .. }))
    {
        return Err(format!(
            "trace fixture `{}` has no run_completed event",
            path.display()
        ));
    }
    Ok(TraceFixture { path, events })
}

pub(super) fn evaluate_trace_assertion(
    assertion: &IrEvalAssert,
    fixture: &TraceFixture,
) -> Result<Option<String>, String> {
    match assertion {
        IrEvalAssert::Called { name, .. } => {
            if event_index(&fixture.events, name).is_some() {
                Ok(Some(format!(
                    "`{name}` appears in trace fixture `{}`",
                    fixture.path.display()
                )))
            } else {
                Err(format!(
                    "`{name}` was not called in trace fixture `{}`",
                    fixture.path.display()
                ))
            }
        }
        IrEvalAssert::Approved { label, .. } => {
            if fixture.events.iter().any(|event| approval_matches(event, label)) {
                Ok(Some(format!(
                    "`{label}` was approved in trace fixture `{}`",
                    fixture.path.display()
                )))
            } else {
                Err(format!(
                    "`{label}` approval was not present in trace fixture `{}`",
                    fixture.path.display()
                ))
            }
        }
        IrEvalAssert::Cost { op, bound, .. } => {
            let total = trace_cost_usd(&fixture.events);
            if compare_number(total, *op, *bound) {
                Ok(Some(format!(
                    "trace cost {total:.6} satisfied {:?} {bound:.6}",
                    op
                )))
            } else {
                Err(format!(
                    "trace cost {total:.6} did not satisfy {:?} {bound:.6}",
                    op
                ))
            }
        }
        IrEvalAssert::Ordering {
            before_name,
            after_name,
            ..
        } => {
            let before = event_index(&fixture.events, before_name);
            let after = event_index(&fixture.events, after_name);
            match (before, after) {
                (Some(left), Some(right)) if left < right => Ok(Some(format!(
                    "`{before_name}` appears before `{after_name}` in trace fixture `{}`",
                    fixture.path.display()
                ))),
                (Some(left), Some(right)) => Err(format!(
                    "`{before_name}` appears at event {left}, not before `{after_name}` at event {right}"
                )),
                (None, _) => Err(format!("`{before_name}` was not called in trace fixture")),
                (_, None) => Err(format!("`{after_name}` was not called in trace fixture")),
            }
        }
        _ => Ok(None),
    }
}

fn resolve_trace_path(spec: &str, root: &Path) -> PathBuf {
    let path = PathBuf::from(spec);
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}

fn event_index(events: &[TraceEvent], name: &str) -> Option<usize> {
    events.iter().position(|event| match event {
        TraceEvent::RunStarted { agent, .. } => agent == name,
        TraceEvent::ToolCall { tool, .. } => tool == name,
        TraceEvent::LlmCall { prompt, .. } => prompt == name,
        _ => false,
    })
}

fn approval_matches(event: &TraceEvent, label: &str) -> bool {
    match event {
        TraceEvent::ApprovalRequest { label: event_label, .. }
        | TraceEvent::ApprovalResponse {
            label: event_label, ..
        } => event_label == label,
        TraceEvent::ApprovalDecision { site, .. } => site == label,
        _ => false,
    }
}

fn trace_cost_usd(events: &[TraceEvent]) -> f64 {
    events.iter().fold(0.0, |total, event| {
        total
            + match event {
                TraceEvent::ModelSelected { cost_estimate, .. } => {
                    if cost_estimate.is_finite() && *cost_estimate > 0.0 {
                        *cost_estimate
                    } else {
                        0.0
                    }
                }
                TraceEvent::HostEvent { payload, .. } => payload
                    .get("cost_usd")
                    .and_then(|value| value.as_f64())
                    .filter(|value| value.is_finite() && *value > 0.0)
                    .unwrap_or(0.0),
                _ => 0.0,
            }
    })
}

fn compare_number(left: f64, op: BinaryOp, right: f64) -> bool {
    match op {
        BinaryOp::Eq => (left - right).abs() <= f64::EPSILON,
        BinaryOp::NotEq => (left - right).abs() > f64::EPSILON,
        BinaryOp::Lt => left < right,
        BinaryOp::LtEq => left <= right,
        BinaryOp::Gt => left > right,
        BinaryOp::GtEq => left >= right,
        _ => false,
    }
}
