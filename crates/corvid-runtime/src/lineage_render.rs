//! Text rendering for Phase 40 lineage events.

use crate::lineage::{validate_lineage, LineageEvent};
use std::collections::BTreeMap;

pub fn render_lineage_tree(events: &[LineageEvent]) -> String {
    let validation = validate_lineage(events);
    let mut out = String::new();
    if !validation.complete {
        out.push_str("lineage: incomplete\n");
        for violation in validation.violations {
            out.push_str("- ");
            out.push_str(&violation);
            out.push('\n');
        }
        return out;
    }
    let mut by_parent: BTreeMap<&str, Vec<&LineageEvent>> = BTreeMap::new();
    let mut root = None;
    for event in events {
        if event.parent_span_id.is_empty() {
            root = Some(event);
        } else {
            by_parent
                .entry(event.parent_span_id.as_str())
                .or_default()
                .push(event);
        }
    }
    if let Some(root) = root {
        render_node(root, &by_parent, 0, &mut out);
    }
    out
}

fn render_node(
    event: &LineageEvent,
    by_parent: &BTreeMap<&str, Vec<&LineageEvent>>,
    depth: usize,
    out: &mut String,
) {
    out.push_str(&"  ".repeat(depth));
    out.push_str("- ");
    out.push_str(kind_label(event));
    out.push(' ');
    out.push_str(&event.name);
    out.push_str(" [");
    out.push_str(status_label(event));
    out.push(']');
    if event.latency_ms > 0 {
        out.push_str(&format!(" latency={}ms", event.latency_ms));
    }
    if event.cost_usd > 0.0 {
        out.push_str(&format!(" cost=${:.4}", event.cost_usd));
    }
    if !event.guarantee_id.is_empty() {
        out.push_str(" guarantee=");
        out.push_str(&event.guarantee_id);
    }
    if !event.approval_id.is_empty() {
        out.push_str(" approval=");
        out.push_str(&event.approval_id);
    }
    if !event.replay_key.is_empty() {
        out.push_str(" replay=");
        out.push_str(&event.replay_key);
    }
    if !event.data_classes.is_empty() {
        out.push_str(" data=");
        out.push_str(&event.data_classes.join(","));
    }
    out.push('\n');

    if let Some(children) = by_parent.get(event.span_id.as_str()) {
        let mut sorted = children.clone();
        sorted.sort_by(|left, right| {
            left.started_ms
                .cmp(&right.started_ms)
                .then_with(|| left.kind.cmp(&right.kind))
                .then_with(|| left.name.cmp(&right.name))
        });
        for child in sorted {
            render_node(child, by_parent, depth + 1, out);
        }
    }
}

fn kind_label(event: &LineageEvent) -> &'static str {
    match event.kind {
        crate::lineage::LineageKind::Route => "route",
        crate::lineage::LineageKind::Job => "job",
        crate::lineage::LineageKind::Agent => "agent",
        crate::lineage::LineageKind::Prompt => "prompt",
        crate::lineage::LineageKind::Tool => "tool",
        crate::lineage::LineageKind::Approval => "approval",
        crate::lineage::LineageKind::Db => "db",
        crate::lineage::LineageKind::Retry => "retry",
        crate::lineage::LineageKind::Error => "error",
        crate::lineage::LineageKind::Eval => "eval",
        crate::lineage::LineageKind::Review => "review",
    }
}

fn status_label(event: &LineageEvent) -> &'static str {
    match event.status {
        crate::lineage::LineageStatus::Ok => "ok",
        crate::lineage::LineageStatus::Failed => "failed",
        crate::lineage::LineageStatus::Denied => "denied",
        crate::lineage::LineageStatus::PendingReview => "pending_review",
        crate::lineage::LineageStatus::Replayed => "replayed",
        crate::lineage::LineageStatus::Redacted => "redacted",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lineage::{LineageEvent, LineageKind, LineageStatus};

    #[test]
    fn renderer_prints_tree_with_contract_context() {
        let mut route = LineageEvent::root("trace-1", LineageKind::Route, "POST /actions", 10)
            .finish(LineageStatus::Ok, 30);
        route.replay_key = "route:trace-1".to_string();
        let mut tool = LineageEvent::child(&route, LineageKind::Tool, "send_email", 0, 15)
            .finish(LineageStatus::Ok, 25);
        tool.cost_usd = 0.02;
        tool.approval_id = "approval-1".to_string();
        tool.guarantee_id = "approval.reachable_entrypoints_require_contract".to_string();
        tool.data_classes = vec!["private".to_string()];
        let approval = LineageEvent::child(&tool, LineageKind::Approval, "SendEmail", 0, 16)
            .finish(LineageStatus::Ok, 20);

        let rendered = render_lineage_tree(&[route, tool, approval]);
        assert!(rendered.contains("- route POST /actions [ok] latency=20ms"));
        assert!(rendered.contains("  - tool send_email [ok] latency=10ms cost=$0.0200"));
        assert!(rendered.contains("guarantee=approval.reachable_entrypoints_require_contract"));
        assert!(rendered.contains("approval=approval-1"));
        assert!(rendered.contains("data=private"));
        assert!(rendered.contains("    - approval SendEmail [ok] latency=4ms"));
    }

    #[test]
    fn renderer_surfaces_incomplete_lineage() {
        let route = LineageEvent::root("trace-1", LineageKind::Route, "GET /", 1);
        let second = LineageEvent::root("trace-1", LineageKind::Job, "daily", 2);
        let rendered = render_lineage_tree(&[route, second]);
        assert!(rendered.starts_with("lineage: incomplete"));
        assert!(rendered.contains("expected_one_root"));
    }
}
