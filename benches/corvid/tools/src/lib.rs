use corvid_runtime::abi::{CorvidString, IntoCorvidAbi};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

type Queues = HashMap<String, VecDeque<CorvidString>>;
type LatencyQueues = HashMap<String, VecDeque<u64>>;

static BENCH_TOOL_WAIT_NS: AtomicU64 = AtomicU64::new(0);

#[no_mangle]
pub extern "C" fn corvid_bench_tool_wait_ns() -> u64 {
    BENCH_TOOL_WAIT_NS.load(Ordering::Relaxed)
}

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
                                Value::String(s) => s.into_corvid_abi(),
                                other => other.to_string().into_corvid_abi(),
                            })
                            .collect(),
                        Value::String(s) => VecDeque::from([s.into_corvid_abi()]),
                        other => VecDeque::from([other.to_string().into_corvid_abi()]),
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
                        Value::Array(values) => values.into_iter().filter_map(|v| v.as_u64()).collect(),
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

fn maybe_sleep_sync(tool: &str) {
    let latency = {
        let mut queues = latencies().lock().unwrap();
        queues
            .get_mut(tool)
            .and_then(|queue| queue.pop_front())
            .unwrap_or(0)
    };
    if latency > 0 {
        let start = Instant::now();
        std::thread::sleep(Duration::from_millis(latency));
        let actual_ms = start.elapsed().as_secs_f64() * 1000.0;
        BENCH_TOOL_WAIT_NS.fetch_add((actual_ms * 1_000_000.0) as u64, Ordering::Relaxed);
    }
}

fn next_response(tool: &str) -> CorvidString {
    let mut queues = responses().lock().unwrap();
    queues
        .get_mut(tool)
        .and_then(|queue| queue.pop_front())
        .unwrap_or_else(|| panic!("corvid_bench_tools: no queued response for `{tool}`"))
}

#[no_mangle]
pub extern "C" fn __corvid_tool_lookup_customer_profile(customer_id: CorvidString) -> CorvidString {
    let _ = customer_id;
    maybe_sleep_sync("lookup_customer_profile");
    next_response("lookup_customer_profile")
}

#[no_mangle]
pub extern "C" fn __corvid_tool_fetch_open_orders(customer_id: CorvidString) -> CorvidString {
    let _ = customer_id;
    maybe_sleep_sync("fetch_open_orders");
    next_response("fetch_open_orders")
}

#[no_mangle]
pub extern "C" fn __corvid_tool_fetch_shipment_status(order_id: CorvidString, attempt: i64) -> CorvidString {
    let _ = order_id;
    let _ = attempt;
    maybe_sleep_sync("fetch_shipment_status");
    next_response("fetch_shipment_status")
}

#[no_mangle]
pub extern "C" fn __corvid_tool_issue_refund(proposal: CorvidString) -> CorvidString {
    let _ = proposal;
    maybe_sleep_sync("issue_refund");
    next_response("issue_refund")
}

#[no_mangle]
pub extern "C" fn __corvid_tool_lookup_customer_ticket_state(customer_id: CorvidString) -> CorvidString {
    let _ = customer_id;
    maybe_sleep_sync("lookup_customer_ticket_state");
    next_response("lookup_customer_ticket_state")
}

#[no_mangle]
pub extern "C" fn __corvid_tool_fetch_escalation_policy(priority: CorvidString) -> CorvidString {
    let _ = priority;
    maybe_sleep_sync("fetch_escalation_policy");
    next_response("fetch_escalation_policy")
}
