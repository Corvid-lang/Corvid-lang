use corvid_trace_schema::TraceEvent;

#[derive(Debug, Clone)]
pub struct ReplayDivergence {
    pub step: usize,
    pub expected: TraceEvent,
    pub got_kind: &'static str,
    pub got_description: String,
}

impl std::fmt::Display for ReplayDivergence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "replay divergence at step {}: expected {:?}, got {} ({})",
            self.step, self.expected, self.got_kind, self.got_description
        )
    }
}

impl std::error::Error for ReplayDivergence {}
