use corvid_trace_schema::TraceEvent;

#[derive(Debug, Clone)]
pub struct ReplayDivergence {
    pub step: usize,
    pub expected: TraceEvent,
    pub got_kind: &'static str,
    pub got_description: String,
}

impl ReplayDivergence {
    /// Stable id of the public Corvid guarantee this runtime error
    /// enforces — `replay.deterministic_pure_path`. The compile-time
    /// portion of the same guarantee is enforced by
    /// `TypeErrorKind::NonReplayableCall` /
    /// `TypeErrorKind::NonDeterministicCall` in `corvid-types`;
    /// this runtime divergence catches the cases the compile-time
    /// check could not statically prove.
    pub const fn guarantee_id(&self) -> &'static str {
        "replay.deterministic_pure_path"
    }
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
