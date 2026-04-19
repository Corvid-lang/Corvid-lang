use super::diverge::ReplayDivergence;
use super::substitute::is_dispatch_metadata;
use corvid_trace_schema::TraceEvent;

#[derive(Debug, Clone)]
pub(crate) struct TraceCursor {
    next: usize,
}

impl TraceCursor {
    pub(crate) fn new(next: usize) -> Self {
        Self { next }
    }

    pub(crate) fn current_step(&self) -> usize {
        self.next
    }

    pub(crate) fn expect_next<F>(
        &mut self,
        events: &[TraceEvent],
        predicate: F,
        got_kind: &'static str,
        got_description: String,
    ) -> Result<TraceEvent, ReplayDivergence>
    where
        F: FnOnce(&TraceEvent) -> bool,
    {
        self.skip_dispatch_metadata(events);
        let expected = events
            .get(self.next)
            .cloned()
            .unwrap_or_else(|| TraceEvent::RunCompleted {
                ts_ms: 0,
                run_id: "<eof>".into(),
                ok: false,
                result: None,
                error: Some("unexpected end of trace".into()),
            });
        if predicate(&expected) {
            self.next += 1;
            Ok(expected)
        } else {
            Err(ReplayDivergence {
                step: self.next,
                expected,
                got_kind,
                got_description,
            })
        }
    }

    pub(crate) fn finish(&mut self, events: &[TraceEvent]) -> Result<(), ReplayDivergence> {
        self.skip_dispatch_metadata(events);
        if self.next == events.len() {
            return Ok(());
        }
        Err(ReplayDivergence {
            step: self.next,
            expected: events[self.next].clone(),
            got_kind: "program_end",
            got_description: "program finished before trace was exhausted".into(),
        })
    }

    fn skip_dispatch_metadata(&mut self, events: &[TraceEvent]) {
        while let Some(event) = events.get(self.next) {
            if is_dispatch_metadata(event) {
                self.next += 1;
            } else {
                break;
            }
        }
    }
}
