//! Stream value family — `StreamValue` (the user-facing handle),
//! `StreamSender` (the driver-side push handle), and the typed
//! per-chunk record `StreamChunk` carrying value + cost +
//! confidence + token count for budget accounting and replay.
//!
//! The split between `Bounded` and `Unbounded` mpsc receivers /
//! senders backs the language's two `BackpressurePolicy`
//! variants — `block` (bounded; producer awaits when full) and
//! `drop_oldest` / `drop_newest` (unbounded; producer always
//! makes progress). The `StreamSenderKind` enum captures the
//! two-channel-flavor split so `StreamSender::send_chunk` can
//! pick the right `mpsc::Sender` method.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, Mutex as AsyncMutex};

use corvid_ast::BackpressurePolicy;
use corvid_runtime::ProvenanceChain;

use super::{
    value_confidence, InterpError, ResumeTokenValue, StreamResumeContext, Value,
    UNBOUNDED_STREAM_WARN_THRESHOLD,
};


pub struct StreamValue(Arc<StreamInner>);

struct StreamInner {
    receiver: AsyncMutex<StreamReceiver>,
    backpressure: BackpressurePolicy,
    provenance: Mutex<ProvenanceChain>,
    history: Mutex<Vec<StreamChunk>>,
    resume_context: Mutex<Option<StreamResumeContext>>,
    pending: AtomicUsize,
    warned_unbounded: AtomicBool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StreamChunk {
    pub value: Value,
    pub cost: f64,
    pub confidence: f64,
    pub tokens: u64,
}

enum StreamReceiver {
    Bounded(mpsc::Receiver<Result<StreamChunk, InterpError>>),
    Unbounded(mpsc::UnboundedReceiver<Result<StreamChunk, InterpError>>),
}

pub(crate) struct StreamSender {
    inner: Arc<StreamInner>,
    kind: StreamSenderKind,
}

enum StreamSenderKind {
    Bounded(mpsc::Sender<Result<StreamChunk, InterpError>>),
    Unbounded(mpsc::UnboundedSender<Result<StreamChunk, InterpError>>),
}

impl StreamValue {
    pub(crate) fn channel(backpressure: BackpressurePolicy) -> (StreamSender, Self) {
        match backpressure {
            BackpressurePolicy::Bounded(size) => {
                let (sender, receiver) = mpsc::channel(size as usize);
                let inner = Arc::new(StreamInner {
                    receiver: AsyncMutex::new(StreamReceiver::Bounded(receiver)),
                    backpressure: BackpressurePolicy::Bounded(size),
                    provenance: Mutex::new(ProvenanceChain::new()),
                    history: Mutex::new(Vec::new()),
                    resume_context: Mutex::new(None),
                    pending: AtomicUsize::new(0),
                    warned_unbounded: AtomicBool::new(false),
                });
                let stream = Self(Arc::clone(&inner));
                let sender = StreamSender {
                    inner,
                    kind: StreamSenderKind::Bounded(sender),
                };
                (sender, stream)
            }
            BackpressurePolicy::PullsFrom(source) => {
                let (sender, receiver) = mpsc::channel(1);
                let inner = Arc::new(StreamInner {
                    receiver: AsyncMutex::new(StreamReceiver::Bounded(receiver)),
                    backpressure: BackpressurePolicy::PullsFrom(source),
                    provenance: Mutex::new(ProvenanceChain::new()),
                    history: Mutex::new(Vec::new()),
                    resume_context: Mutex::new(None),
                    pending: AtomicUsize::new(0),
                    warned_unbounded: AtomicBool::new(false),
                });
                let stream = Self(Arc::clone(&inner));
                let sender = StreamSender {
                    inner,
                    kind: StreamSenderKind::Bounded(sender),
                };
                (sender, stream)
            }
            BackpressurePolicy::Unbounded => {
                let (sender, receiver) = mpsc::unbounded_channel();
                let inner = Arc::new(StreamInner {
                    receiver: AsyncMutex::new(StreamReceiver::Unbounded(receiver)),
                    backpressure: BackpressurePolicy::Unbounded,
                    provenance: Mutex::new(ProvenanceChain::new()),
                    history: Mutex::new(Vec::new()),
                    resume_context: Mutex::new(None),
                    pending: AtomicUsize::new(0),
                    warned_unbounded: AtomicBool::new(false),
                });
                let stream = Self(Arc::clone(&inner));
                let sender = StreamSender {
                    inner,
                    kind: StreamSenderKind::Unbounded(sender),
                };
                (sender, stream)
            }
        }
    }

    pub async fn next(&self) -> Option<Result<Value, InterpError>> {
        self.next_chunk()
            .await
            .map(|item| item.map(|chunk| chunk.value))
    }

    pub(crate) async fn next_chunk(&self) -> Option<Result<StreamChunk, InterpError>> {
        let mut receiver = self.0.receiver.lock().await;
        let item = match &mut *receiver {
            StreamReceiver::Bounded(rx) => rx.recv().await,
            StreamReceiver::Unbounded(rx) => rx.recv().await,
        };
        if item.is_some() {
            self.0.pending.fetch_sub(1, Ordering::AcqRel);
        }
        if let Some(Ok(chunk)) = &item {
            self.0
                .history
                .lock()
                .expect("stream history poisoned")
                .push(chunk.clone());
            if let Some(chain) = chunk.provenance() {
                self.0
                    .provenance
                    .lock()
                    .expect("stream provenance poisoned")
                    .merge(chain);
            }
        }
        item
    }

    pub fn provenance(&self) -> ProvenanceChain {
        self.0
            .provenance
            .lock()
            .expect("stream provenance poisoned")
            .clone()
    }

    pub fn backpressure(&self) -> &BackpressurePolicy {
        &self.0.backpressure
    }

    pub(crate) fn set_resume_context(&self, context: StreamResumeContext) {
        *self
            .0
            .resume_context
            .lock()
            .expect("stream resume context poisoned") = Some(context);
    }

    pub(crate) fn resume_token(&self) -> Option<ResumeTokenValue> {
        let context = self
            .0
            .resume_context
            .lock()
            .expect("stream resume context poisoned")
            .clone()?;
        let delivered = self
            .0
            .history
            .lock()
            .expect("stream history poisoned")
            .clone();
        Some(ResumeTokenValue {
            prompt_name: context.prompt_name,
            args: context.args,
            delivered,
            provider_session: context.provider_session,
        })
    }

    pub(super) fn backpressure_label(&self) -> String {
        self.backpressure().label()
    }
}

impl Clone for StreamValue {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl PartialEq for StreamValue {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}


impl StreamSender {
    pub async fn send(&self, item: Result<Value, InterpError>) -> bool {
        self.send_chunk(item.map(StreamChunk::new)).await
    }

    pub(crate) async fn send_chunk(&self, item: Result<StreamChunk, InterpError>) -> bool {
        let pending_after_send = self.inner.pending.fetch_add(1, Ordering::AcqRel) + 1;
        match &self.kind {
            StreamSenderKind::Bounded(sender) => {
                if sender.send(item).await.is_err() {
                    self.inner.pending.fetch_sub(1, Ordering::AcqRel);
                    return false;
                }
            }
            StreamSenderKind::Unbounded(sender) => {
                if sender.send(item).is_err() {
                    self.inner.pending.fetch_sub(1, Ordering::AcqRel);
                    return false;
                }
                if pending_after_send > UNBOUNDED_STREAM_WARN_THRESHOLD
                    && !self.inner.warned_unbounded.swap(true, Ordering::AcqRel)
                {
                    eprintln!(
                        "warning: unbounded stream buffer exceeded {} queued items",
                        UNBOUNDED_STREAM_WARN_THRESHOLD
                    );
                }
            }
        }
        true
    }
}

impl StreamChunk {
    pub fn new(value: Value) -> Self {
        Self {
            confidence: value_confidence(&value),
            value,
            cost: 0.0,
            tokens: 0,
        }
    }

    pub fn with_metrics(value: Value, cost: f64, confidence: f64, tokens: u64) -> Self {
        Self {
            value,
            cost,
            confidence,
            tokens,
        }
    }

    pub fn provenance(&self) -> Option<&ProvenanceChain> {
        match &self.value {
            Value::Grounded(grounded) => Some(&grounded.provenance),
            _ => None,
        }
    }
}
