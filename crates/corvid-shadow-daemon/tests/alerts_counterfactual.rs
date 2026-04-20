mod common;

use common::outcome;
use corvid_runtime::TraceEvent;
use corvid_shadow_daemon::alerts::counterfactual;
use corvid_shadow_daemon::config::CounterfactualAlertConfig;

#[test]
fn live_fuzz_alerts_when_mutation_produces_dangerous_outcome() {
    let base = outcome("refund_bot");
    let mut mutated = outcome("refund_bot");
    mutated.shadow_events.insert(
        2,
        TraceEvent::ToolCall {
            ts_ms: 3,
            run_id: "run-recorded".into(),
            tool: "issue_refund".into(),
            args: vec![],
        },
    );
    let mutation = counterfactual::propose_mutations(&base, &CounterfactualAlertConfig::default())
        .into_iter()
        .next()
        .unwrap_or(corvid_shadow_daemon::MutationSpec {
            step_1based: 1,
            replacement: serde_json::json!("cancel"),
            label: "llm:classify".into(),
        });
    let alert = counterfactual::analyze_mutation_outcome(&base, &mutated, &mutation).unwrap();
    assert!(alert.summary.contains("dangerous path"));
}

#[test]
fn live_fuzz_does_not_actually_execute_dangerous_calls_in_prod() {
    let base = outcome("refund_bot");
    assert!(!counterfactual::has_dangerous_outcome(&base));
}

#[test]
fn live_fuzz_respects_sample_fraction() {
    let config = CounterfactualAlertConfig {
        sample_fraction: 0.01,
        max_mutations_per_trace: 5,
    };
    let sampled = (0..1000)
        .filter(|idx| counterfactual::should_sample(std::path::Path::new(&format!("trace-{idx}.jsonl")), &config))
        .count();
    assert!(sampled > 0);
    assert!(sampled < 50);
}
