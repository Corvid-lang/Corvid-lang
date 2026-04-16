use corvid_macros::tool;
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

type Queues = HashMap<String, VecDeque<String>>;
type LatencyQueues = HashMap<String, VecDeque<u64>>;

fn parse_string_queues(var: &str) -> Queues {
    std::env::var(var)
        .ok()
        .and_then(|raw| serde_json::from_str::<HashMap<String, Value>>(&raw).ok())
        .map(|map| {
            map.into_iter()
                .map(|(name, value)| {
                    let queue = match value {
                        Value::Array(values) => values
                            .into_iter()
                            .map(|v| match v {
                                Value::String(s) => s,
                                other => other.to_string(),
                            })
                            .collect(),
                        Value::String(s) => VecDeque::from([s]),
                        other => VecDeque::from([other.to_string()]),
                    };
                    (name, queue)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_latency_queues(var: &str) -> LatencyQueues {
    std::env::var(var)
        .ok()
        .and_then(|raw| serde_json::from_str::<HashMap<String, Value>>(&raw).ok())
        .map(|map| {
            map.into_iter()
                .map(|(name, value)| {
                    let queue = match value {
                        Value::Array(values) => values
                            .into_iter()
                            .filter_map(|v| v.as_u64())
                            .collect(),
                        other => other
                            .as_u64()
                            .map(|v| VecDeque::from([v]))
                            .unwrap_or_default(),
                    };
                    (name, queue)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn responses() -> &'static Mutex<Queues> {
    static RESPONSES: OnceLock<Mutex<Queues>> = OnceLock::new();
    RESPONSES.get_or_init(|| Mutex::new(parse_string_queues("CORVID_BENCH_TOOL_RESPONSES")))
}

fn latencies() -> &'static Mutex<LatencyQueues> {
    static LATENCIES: OnceLock<Mutex<LatencyQueues>> = OnceLock::new();
    LATENCIES.get_or_init(|| Mutex::new(parse_latency_queues("CORVID_BENCH_TOOL_LATENCIES_MS")))
}

fn profile_enabled() -> bool {
    std::env::var("CORVID_PROFILE_EVENTS").ok().as_deref() == Some("1")
}

async fn maybe_sleep(tool: &str) {
    let latency = {
        let mut queues = latencies().lock().unwrap();
        queues
            .get_mut(tool)
            .and_then(|queue| queue.pop_front())
            .unwrap_or(0)
    };
    if latency > 0 {
        let start = Instant::now();
        tokio::time::sleep(Duration::from_millis(latency)).await;
        if profile_enabled() {
            let event = serde_json::json!({
                "kind": "wait",
                "source_kind": "tool",
                "name": tool,
                "nominal_ms": latency,
                "actual_ms": start.elapsed().as_secs_f64() * 1000.0,
            });
            eprintln!("CORVID_PROFILE_JSON={event}");
        }
    }
}

fn next_response(tool: &str) -> String {
    let mut queues = responses().lock().unwrap();
    queues
        .get_mut(tool)
        .and_then(|queue| queue.pop_front())
        .unwrap_or_else(|| panic!("corvid_bench_tools: no queued response for `{tool}`"))
}

#[tool("lookup_customer_profile")]
async fn lookup_customer_profile(_customer_id: String) -> String {
    maybe_sleep("lookup_customer_profile").await;
    next_response("lookup_customer_profile")
}

#[tool("fetch_open_orders")]
async fn fetch_open_orders(_customer_id: String) -> String {
    maybe_sleep("fetch_open_orders").await;
    next_response("fetch_open_orders")
}

#[tool("fetch_shipment_status")]
async fn fetch_shipment_status(_order_id: String, _attempt: i64) -> String {
    maybe_sleep("fetch_shipment_status").await;
    next_response("fetch_shipment_status")
}

#[tool("issue_refund")]
async fn issue_refund(_proposal: String) -> String {
    maybe_sleep("issue_refund").await;
    next_response("issue_refund")
}

#[tool("lookup_customer_ticket_state")]
async fn lookup_customer_ticket_state(_customer_id: String) -> String {
    maybe_sleep("lookup_customer_ticket_state").await;
    next_response("lookup_customer_ticket_state")
}

#[tool("fetch_escalation_policy")]
async fn fetch_escalation_policy(_priority: String) -> String {
    maybe_sleep("fetch_escalation_policy").await;
    next_response("fetch_escalation_policy")
}
