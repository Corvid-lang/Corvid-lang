use crate::attestation_store::AttestationStore;
use crate::llm::TokenUsage;
use std::cell::RefCell;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

pub const NULL_OBSERVATION_HANDLE: u64 = 0;

#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    pub cost_usd: f64,
    pub latency_ms: u64,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub exceeded_bound: bool,
}

#[derive(Debug)]
struct ActiveObservation {
    started_at: Instant,
    declared_bound_usd: Option<f64>,
    cost_usd: f64,
    tokens_in: u64,
    tokens_out: u64,
}

impl ActiveObservation {
    fn new(declared_bound_usd: Option<f64>) -> Self {
        Self {
            started_at: Instant::now(),
            declared_bound_usd,
            cost_usd: 0.0,
            tokens_in: 0,
            tokens_out: 0,
        }
    }

    fn finish(self) -> Observation {
        Observation {
            cost_usd: self.cost_usd,
            latency_ms: self.started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
            tokens_in: self.tokens_in,
            tokens_out: self.tokens_out,
            exceeded_bound: self
                .declared_bound_usd
                .map(|bound| self.cost_usd > bound)
                .unwrap_or(false),
        }
    }
}

pub struct ObservationScope {
    finished: bool,
}

impl ObservationScope {
    pub fn finish(mut self) -> u64 {
        self.finished = true;
        ACTIVE_OBSERVATIONS.with(|stack| {
            let Some(active) = stack.borrow_mut().pop() else {
                return NULL_OBSERVATION_HANDLE;
            };
            let mut store = store().lock().unwrap();
            store.insert(Arc::new(active.finish()))
        })
    }
}

impl Drop for ObservationScope {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        ACTIVE_OBSERVATIONS.with(|stack| {
            let _ = stack.borrow_mut().pop();
        });
    }
}

pub fn begin_observation(declared_bound_usd: Option<f64>) -> ObservationScope {
    ACTIVE_OBSERVATIONS.with(|stack| {
        stack
            .borrow_mut()
            .push(ActiveObservation::new(declared_bound_usd));
    });
    ObservationScope { finished: false }
}

pub fn record_llm_usage(usage: TokenUsage, cost_usd: f64) {
    ACTIVE_OBSERVATIONS.with(|stack| {
        let mut stack = stack.borrow_mut();
        if stack.is_empty() {
            return;
        }
        for active in stack.iter_mut() {
            active.tokens_in += usage.prompt_tokens as u64;
            active.tokens_out += usage.completion_tokens as u64;
            if cost_usd.is_finite() && cost_usd > 0.0 {
                active.cost_usd += cost_usd;
            }
        }
    });
}

pub fn cost_usd_for_handle(handle: u64) -> Option<f64> {
    if handle == NULL_OBSERVATION_HANDLE {
        return None;
    }
    let store = store().lock().unwrap();
    store.get(handle).map(|observation| observation.cost_usd)
}

pub fn latency_ms_for_handle(handle: u64) -> Option<u64> {
    if handle == NULL_OBSERVATION_HANDLE {
        return None;
    }
    let store = store().lock().unwrap();
    store.get(handle).map(|observation| observation.latency_ms)
}

pub fn tokens_in_for_handle(handle: u64) -> Option<u64> {
    if handle == NULL_OBSERVATION_HANDLE {
        return None;
    }
    let store = store().lock().unwrap();
    store.get(handle).map(|observation| observation.tokens_in)
}

pub fn tokens_out_for_handle(handle: u64) -> Option<u64> {
    if handle == NULL_OBSERVATION_HANDLE {
        return None;
    }
    let store = store().lock().unwrap();
    store.get(handle).map(|observation| observation.tokens_out)
}

pub fn exceeded_bound_for_handle(handle: u64) -> Option<bool> {
    if handle == NULL_OBSERVATION_HANDLE {
        return None;
    }
    let store = store().lock().unwrap();
    store.get(handle).map(|observation| observation.exceeded_bound)
}

pub fn release_handle(handle: u64) -> bool {
    if handle == NULL_OBSERVATION_HANDLE {
        return true;
    }
    let mut store = store().lock().unwrap();
    store.remove(handle)
}

pub fn emit_debug_leak_warning() {
    let store = store().lock().unwrap();
    store.emit_debug_leak_warning();
}

thread_local! {
    static ACTIVE_OBSERVATIONS: RefCell<Vec<ActiveObservation>> = const { RefCell::new(Vec::new()) };
}

static STORE: OnceLock<Mutex<AttestationStore<Observation>>> = OnceLock::new();

fn store() -> &'static Mutex<AttestationStore<Observation>> {
    STORE.get_or_init(|| Mutex::new(AttestationStore::new("observation handle store")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_zero_is_noop() {
        assert!(release_handle(NULL_OBSERVATION_HANDLE));
    }

    #[test]
    fn observation_roundtrips_all_fields() {
        let scope = begin_observation(Some(0.5));
        record_llm_usage(
            TokenUsage {
                prompt_tokens: 11,
                completion_tokens: 7,
                total_tokens: 18,
            },
            0.25,
        );
        let handle = scope.finish();
        assert_ne!(handle, NULL_OBSERVATION_HANDLE);
        assert_eq!(tokens_in_for_handle(handle), Some(11));
        assert_eq!(tokens_out_for_handle(handle), Some(7));
        assert_eq!(exceeded_bound_for_handle(handle), Some(false));
        assert_eq!(cost_usd_for_handle(handle), Some(0.25));
        assert!(latency_ms_for_handle(handle).unwrap() <= 1_000);
        assert!(release_handle(handle));
    }

    #[test]
    fn exceeded_bound_tracks_declared_budget() {
        let scope = begin_observation(Some(0.1));
        record_llm_usage(TokenUsage::default(), 0.25);
        let handle = scope.finish();
        assert_eq!(exceeded_bound_for_handle(handle), Some(true));
        assert!(release_handle(handle));
    }

    #[test]
    fn stale_handle_fails_after_release() {
        let handle = begin_observation(None).finish();
        assert!(release_handle(handle));
        assert!(!release_handle(handle));
    }
}
