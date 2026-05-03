use super::*;

#[derive(Debug, Clone)]
pub(super) struct ReplayMutation {
    pub(super) step_1based: usize,
    pub(super) replacement: serde_json::Value,
    pub(super) report: Arc<Mutex<ReplayMutationReport>>,
    pub(super) state: Arc<Mutex<ReplayMutationState>>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct ReplayMutationState {
    next_step: usize,
}

impl ReplayMutation {
    pub(super) fn next_step(&self) -> usize {
        let mut state = self.state.lock().unwrap();
        state.next_step += 1;
        state.next_step
    }
}
