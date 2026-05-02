//! Runtime value representation.
//!
//! Primitives copy by value. Cycle-capable composites (`Struct`, `List`,
//! `Result*`, `OptionSome`) ride on VM-owned heap cells with explicit
//! retain/release bookkeeping so the interpreter can own its refcount
//! semantics instead of delegating them to `Arc`.
//!
//! `String` intentionally stays `Arc<str>` for now because it is a leaf
//! payload with no outgoing refcounted edges. If a future string-like type
//! ever gains outgoing refcounted edges (rope fragments, parent-backed
//! string views, lazy concat nodes), it must migrate to `HeapHandle` style
//! ownership and participate in the VM collector.

use crate::cycle_collector;
use crate::errors::InterpError;
use corvid_ast::BackpressurePolicy;
use corvid_resolve::DefId;
use corvid_runtime::{ProvenanceChain, ProvenanceEntry, ProvenanceKind};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use tokio::sync::{mpsc, Mutex as AsyncMutex};

mod display;
mod weak;
pub use display::value_confidence;
pub use weak::{ListWeakValue, StructWeakValue, WeakValue};

/// A runtime value.
#[derive(Debug)]
pub enum Value {
    Int(i64),
    Float(f64),
    String(Arc<str>),
    Bool(bool),
    Nothing,
    Struct(StructValue),
    List(ListValue),
    Weak(WeakValue),
    ResultOk(BoxedValue),
    ResultErr(BoxedValue),
    OptionSome(BoxedValue),
    OptionNone,
    Grounded(GroundedValue),
    Partial(PartialValue),
    ResumeToken(ResumeTokenValue),
    Stream(StreamValue),
}

const UNBOUNDED_STREAM_WARN_THRESHOLD: usize = 1024;

/// A value with a provenance chain proving it derives from a grounded source.
#[derive(Debug, Clone)]
pub struct GroundedValue {
    pub inner: BoxedValue,
    pub provenance: ProvenanceChain,
    /// LLM-reported or deterministic confidence, composed via Min
    /// through the call graph. Defaults to 1.0 for deterministic tool
    /// results; prompts can set lower values from self-reported
    /// confidence or logprobs.
    pub confidence: f64,
}

impl GroundedValue {
    pub fn new(inner: Value, provenance: ProvenanceChain) -> Self {
        Self {
            inner: BoxedValue::new(inner),
            provenance,
            confidence: 1.0,
        }
    }

    pub fn with_confidence(inner: Value, provenance: ProvenanceChain, confidence: f64) -> Self {
        Self {
            inner: BoxedValue::new(inner),
            provenance,
            confidence: confidence.clamp(0.0, 1.0),
        }
    }

    pub fn sources(&self) -> &[ProvenanceEntry] {
        &self.provenance.entries
    }

    pub fn unwrap_with_reason(self, reason: &str) -> (Value, ProvenanceEntry) {
        let severed = ProvenanceEntry {
            kind: ProvenanceKind::Severed { reason: reason.to_string() },
            name: "unwrap".to_string(),
            timestamp_ms: 0,
        };
        (self.inner.get(), severed)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PartialFieldValue {
    Complete(Value),
    Streaming,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PartialValue {
    type_id: DefId,
    type_name: String,
    fields: HashMap<String, PartialFieldValue>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResumeTokenValue {
    pub(crate) prompt_name: String,
    pub(crate) args: Vec<Value>,
    pub(crate) delivered: Vec<StreamChunk>,
    pub(crate) provider_session: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct StreamResumeContext {
    pub prompt_name: String,
    pub args: Vec<Value>,
    pub provider_session: Option<String>,
}

impl PartialValue {
    pub fn new(
        type_id: DefId,
        type_name: impl Into<String>,
        fields: impl IntoIterator<Item = (String, PartialFieldValue)>,
    ) -> Self {
        Self {
            type_id,
            type_name: type_name.into(),
            fields: fields.into_iter().collect(),
        }
    }

    pub fn type_name(&self) -> &str {
        &self.type_name
    }

    pub fn get_field(&self, field: &str) -> Option<Value> {
        match self.fields.get(field)? {
            PartialFieldValue::Complete(value) => {
                Some(Value::OptionSome(BoxedValue::new(value.clone())))
            }
            PartialFieldValue::Streaming => Some(Value::OptionNone),
        }
    }

    pub fn with_fields<R>(&self, f: impl FnOnce(&HashMap<String, PartialFieldValue>) -> R) -> R {
        f(&self.fields)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum Color {
    Black = 0,
    Gray = 1,
    White = 2,
    Purple = 3,
}

#[derive(Debug)]
struct HeapMeta {
    strong: AtomicUsize,
    shadow: AtomicUsize,
    color: AtomicU8,
    buffered: AtomicBool,
}

impl HeapMeta {
    fn new() -> Self {
        Self {
            strong: AtomicUsize::new(1),
            shadow: AtomicUsize::new(0),
            color: AtomicU8::new(Color::Black as u8),
            buffered: AtomicBool::new(false),
        }
    }

    fn retain(&self) {
        self.strong.fetch_add(1, Ordering::Relaxed);
    }

    fn release(&self) -> usize {
        self.strong.fetch_sub(1, Ordering::AcqRel)
    }

    fn strong_count(&self) -> usize {
        self.strong.load(Ordering::Acquire)
    }

    fn set_strong(&self, value: usize) {
        self.strong.store(value, Ordering::Release);
    }

    fn shadow_count(&self) -> usize {
        self.shadow.load(Ordering::Acquire)
    }

    fn set_shadow(&self, value: usize) {
        self.shadow.store(value, Ordering::Release);
    }

    fn inc_shadow(&self) {
        self.shadow.fetch_add(1, Ordering::Relaxed);
    }

    fn dec_shadow(&self) {
        let old = self.shadow.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(old > 0, "shadow count underflow");
    }

    fn color(&self) -> Color {
        match self.color.load(Ordering::Acquire) {
            0 => Color::Black,
            1 => Color::Gray,
            2 => Color::White,
            3 => Color::Purple,
            other => panic!("invalid heap color {other}"),
        }
    }

    fn set_color(&self, color: Color) {
        self.color.store(color as u8, Ordering::Release);
    }

    fn buffered(&self) -> bool {
        self.buffered.load(Ordering::Acquire)
    }

    fn set_buffered(&self, value: bool) {
        self.buffered.store(value, Ordering::Release);
    }
}

#[derive(Debug)]
pub(crate) struct StructInner {
    meta: HeapMeta,
    type_id: DefId,
    type_name: String,
    fields: Mutex<Option<HashMap<String, Value>>>,
}

#[derive(Debug)]
pub(crate) struct ListInner {
    meta: HeapMeta,
    items: Mutex<Option<Vec<Value>>>,
}

#[derive(Debug)]
pub(crate) struct BoxedInner {
    meta: HeapMeta,
    value: Mutex<Option<Value>>,
}

#[derive(Debug)]
pub struct StructValue(pub(super) Arc<StructInner>);

#[derive(Debug)]
pub struct ListValue(pub(super) Arc<ListInner>);

#[derive(Debug)]
pub struct BoxedValue(Arc<BoxedInner>);

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



#[derive(Clone)]
pub(crate) enum ObjectRef {
    Struct(Arc<StructInner>),
    List(Arc<ListInner>),
    Boxed(Arc<BoxedInner>),
}

#[derive(Clone)]
pub(crate) enum WeakObjectRef {
    Struct(Weak<StructInner>),
    List(Weak<ListInner>),
    Boxed(Weak<BoxedInner>),
}

impl ObjectRef {
    fn meta(&self) -> &HeapMeta {
        match self {
            ObjectRef::Struct(inner) => &inner.meta,
            ObjectRef::List(inner) => &inner.meta,
            ObjectRef::Boxed(inner) => &inner.meta,
        }
    }

    pub(crate) fn ptr_key(&self) -> usize {
        match self {
            ObjectRef::Struct(inner) => Arc::as_ptr(inner) as usize,
            ObjectRef::List(inner) => Arc::as_ptr(inner) as usize,
            ObjectRef::Boxed(inner) => Arc::as_ptr(inner) as usize,
        }
    }

    pub(crate) fn strong_count(&self) -> usize {
        self.meta().strong_count()
    }

    pub(crate) fn release_strong(&self) -> usize {
        self.meta().release()
    }

    pub(crate) fn set_strong_zero(&self) {
        self.meta().set_strong(0);
    }

    pub(crate) fn shadow_count(&self) -> usize {
        self.meta().shadow_count()
    }

    pub(crate) fn set_shadow(&self, value: usize) {
        self.meta().set_shadow(value);
    }

    pub(crate) fn inc_shadow(&self) {
        self.meta().inc_shadow();
    }

    pub(crate) fn dec_shadow(&self) {
        self.meta().dec_shadow();
    }

    pub(crate) fn color(&self) -> Color {
        self.meta().color()
    }

    pub(crate) fn set_color(&self, color: Color) {
        self.meta().set_color(color);
    }

    pub(crate) fn buffered(&self) -> bool {
        self.meta().buffered()
    }

    pub(crate) fn set_buffered(&self, value: bool) {
        self.meta().set_buffered(value);
    }

    pub(crate) fn downgrade(&self) -> WeakObjectRef {
        match self {
            ObjectRef::Struct(inner) => WeakObjectRef::Struct(Arc::downgrade(inner)),
            ObjectRef::List(inner) => WeakObjectRef::List(Arc::downgrade(inner)),
            ObjectRef::Boxed(inner) => WeakObjectRef::Boxed(Arc::downgrade(inner)),
        }
    }

    pub(crate) fn children(&self) -> Vec<ObjectRef> {
        match self {
            ObjectRef::Struct(inner) => inner
                .fields
                .lock()
                .expect("struct fields lock poisoned")
                .as_ref()
                .map(children_from_map)
                .unwrap_or_default(),
            ObjectRef::List(inner) => inner
                .items
                .lock()
                .expect("list items lock poisoned")
                .as_ref()
                .map(|items| children_from_slice(items))
                .unwrap_or_default(),
            ObjectRef::Boxed(inner) => inner
                .value
                .lock()
                .expect("boxed value lock poisoned")
                .as_ref()
                .and_then(Value::as_object_ref)
                .into_iter()
                .collect(),
        }
    }

    pub(crate) fn free_zero_path(&self) {
        self.set_buffered(false);
        self.set_shadow(0);
        self.set_color(Color::Black);
        self.clear_payload();
    }

    pub(crate) fn prepare_collect(&self) {
        self.set_buffered(false);
        self.set_shadow(0);
        self.set_color(Color::Black);
        self.set_strong_zero();
    }

    pub(crate) fn clear_payload(&self) {
        match self {
            ObjectRef::Struct(inner) => {
                let fields = inner
                    .fields
                    .lock()
                    .expect("struct fields lock poisoned")
                    .take();
                drop(fields);
            }
            ObjectRef::List(inner) => {
                let items = inner
                    .items
                    .lock()
                    .expect("list items lock poisoned")
                    .take();
                drop(items);
            }
            ObjectRef::Boxed(inner) => {
                let value = inner
                    .value
                    .lock()
                    .expect("boxed value lock poisoned")
                    .take();
                drop(value);
            }
        }
    }
}

impl WeakObjectRef {
    pub(crate) fn upgrade(&self) -> Option<ObjectRef> {
        match self {
            WeakObjectRef::Struct(inner) => inner.upgrade().map(ObjectRef::Struct),
            WeakObjectRef::List(inner) => inner.upgrade().map(ObjectRef::List),
            WeakObjectRef::Boxed(inner) => inner.upgrade().map(ObjectRef::Boxed),
        }
    }
}

fn children_from_map(fields: &HashMap<String, Value>) -> Vec<ObjectRef> {
    let mut out = Vec::new();
    for value in fields.values() {
        if let Some(child) = value.as_object_ref() {
            out.push(child);
        }
    }
    out
}

fn children_from_slice(items: &[Value]) -> Vec<ObjectRef> {
    let mut out = Vec::new();
    for value in items {
        if let Some(child) = value.as_object_ref() {
            out.push(child);
        }
    }
    out
}

impl StructValue {
    pub fn new(
        type_id: DefId,
        type_name: impl Into<String>,
        fields: impl IntoIterator<Item = (String, Value)>,
    ) -> Self {
        Self(Arc::new(StructInner {
            meta: HeapMeta::new(),
            type_id,
            type_name: type_name.into(),
            fields: Mutex::new(Some(fields.into_iter().collect())),
        }))
    }

    pub fn type_id(&self) -> DefId {
        self.0.type_id
    }

    pub fn type_name(&self) -> &str {
        &self.0.type_name
    }

    pub fn get_field(&self, field: &str) -> Option<Value> {
        self.0
            .fields
            .lock()
            .expect("struct fields lock poisoned")
            .as_ref()
            .and_then(|fields| fields.get(field).cloned())
    }

    pub fn with_fields<R>(&self, f: impl FnOnce(&HashMap<String, Value>) -> R) -> R {
        let guard = self.0.fields.lock().expect("struct fields lock poisoned");
        let fields = guard.as_ref().expect("struct payload already freed");
        f(fields)
    }

    pub fn ptr_key(&self) -> usize {
        Arc::as_ptr(&self.0) as usize
    }

    #[doc(hidden)]
    pub fn set_field(&self, field: impl Into<String>, value: Value) {
        let mut guard = self.0.fields.lock().expect("struct fields lock poisoned");
        let fields = guard.as_mut().expect("struct payload already freed");
        fields.insert(field.into(), value);
    }

    #[doc(hidden)]
    pub fn strong_count_for_tests(&self) -> usize {
        self.0.meta.strong_count()
    }
}

impl Clone for StructValue {
    fn clone(&self) -> Self {
        self.0.meta.retain();
        Self(self.0.clone())
    }
}

impl Drop for StructValue {
    fn drop(&mut self) {
        cycle_collector::release_object(ObjectRef::Struct(self.0.clone()));
    }
}

impl PartialEq for StructValue {
    fn eq(&self, other: &Self) -> bool {
        self.type_id() == other.type_id()
            && self.with_fields(|a| other.with_fields(|b| a == b))
    }
}

impl ListValue {
    pub fn new(items: impl IntoIterator<Item = Value>) -> Self {
        Self(Arc::new(ListInner {
            meta: HeapMeta::new(),
            items: Mutex::new(Some(items.into_iter().collect())),
        }))
    }

    pub fn len(&self) -> usize {
        self.0
            .items
            .lock()
            .expect("list items lock poisoned")
            .as_ref()
            .expect("list payload already freed")
            .len()
    }

    pub fn iter_cloned(&self) -> Vec<Value> {
        self.0
            .items
            .lock()
            .expect("list items lock poisoned")
            .as_ref()
            .expect("list payload already freed")
            .clone()
    }

    pub fn get(&self, idx: usize) -> Option<Value> {
        self.0
            .items
            .lock()
            .expect("list items lock poisoned")
            .as_ref()
            .and_then(|items| items.get(idx).cloned())
    }

    pub fn ptr_key(&self) -> usize {
        Arc::as_ptr(&self.0) as usize
    }
}

impl Clone for ListValue {
    fn clone(&self) -> Self {
        self.0.meta.retain();
        Self(self.0.clone())
    }
}

impl Drop for ListValue {
    fn drop(&mut self) {
        cycle_collector::release_object(ObjectRef::List(self.0.clone()));
    }
}

impl PartialEq for ListValue {
    fn eq(&self, other: &Self) -> bool {
        self.iter_cloned() == other.iter_cloned()
    }
}

impl BoxedValue {
    pub fn new(value: Value) -> Self {
        Self(Arc::new(BoxedInner {
            meta: HeapMeta::new(),
            value: Mutex::new(Some(value)),
        }))
    }

    pub fn get(&self) -> Value {
        self.0
            .value
            .lock()
            .expect("boxed value lock poisoned")
            .as_ref()
            .expect("boxed payload already freed")
            .clone()
    }

    pub fn ptr_key(&self) -> usize {
        Arc::as_ptr(&self.0) as usize
    }
}

impl Clone for BoxedValue {
    fn clone(&self) -> Self {
        self.0.meta.retain();
        Self(self.0.clone())
    }
}

impl Drop for BoxedValue {
    fn drop(&mut self) {
        cycle_collector::release_object(ObjectRef::Boxed(self.0.clone()));
    }
}

impl PartialEq for BoxedValue {
    fn eq(&self, other: &Self) -> bool {
        self.get() == other.get()
    }
}



impl Clone for Value {
    fn clone(&self) -> Self {
        match self {
            Value::Int(n) => Value::Int(*n),
            Value::Float(x) => Value::Float(*x),
            Value::String(s) => Value::String(s.clone()),
            Value::Bool(b) => Value::Bool(*b),
            Value::Nothing => Value::Nothing,
            Value::Struct(s) => Value::Struct(s.clone()),
            Value::List(items) => Value::List(items.clone()),
            Value::Weak(w) => Value::Weak(w.clone()),
            Value::ResultOk(v) => Value::ResultOk(v.clone()),
            Value::ResultErr(v) => Value::ResultErr(v.clone()),
            Value::OptionSome(v) => Value::OptionSome(v.clone()),
            Value::OptionNone => Value::OptionNone,
            Value::Grounded(g) => Value::Grounded(g.clone()),
            Value::Partial(p) => Value::Partial(p.clone()),
            Value::ResumeToken(token) => Value::ResumeToken(token.clone()),
            Value::Stream(stream) => Value::Stream(stream.clone()),
        }
    }
}

impl Value {
    pub fn type_name(&self) -> String {
        match self {
            Value::Int(_) => "Int".into(),
            Value::Float(_) => "Float".into(),
            Value::String(_) => "String".into(),
            Value::Bool(_) => "Bool".into(),
            Value::Nothing => "Nothing".into(),
            Value::Struct(s) => s.type_name().to_string(),
            Value::List(_) => "List".into(),
            Value::Weak(_) => "Weak".into(),
            Value::ResultOk(_) | Value::ResultErr(_) => "Result".into(),
            Value::OptionSome(_) | Value::OptionNone => "Option".into(),
            Value::Grounded(g) => format!("Grounded<{}>", g.inner.get().type_name()),
            Value::Partial(p) => format!("Partial<{}>", p.type_name()),
            Value::ResumeToken(_) => "ResumeToken".into(),
            Value::Stream(stream) => {
                format!("Stream<{}>", stream.backpressure_label())
            }
        }
    }

    pub fn new_struct(
        type_id: DefId,
        type_name: impl Into<String>,
        fields: impl IntoIterator<Item = (String, Value)>,
    ) -> Value {
        Value::Struct(StructValue::new(type_id, type_name, fields))
    }

    pub fn downgrade(&self) -> Option<WeakValue> {
        match self {
            Value::String(s) => Some(WeakValue::String(Arc::downgrade(s))),
            Value::Struct(s) => Some(WeakValue::Struct(StructWeakValue(Arc::downgrade(&s.0)))),
            Value::List(items) => Some(WeakValue::List(ListWeakValue(Arc::downgrade(&items.0)))),
            _ => None,
        }
    }

    pub(crate) fn as_object_ref(&self) -> Option<ObjectRef> {
        match self {
            Value::Struct(s) => Some(ObjectRef::Struct(s.0.clone())),
            Value::List(items) => Some(ObjectRef::List(items.0.clone())),
            Value::ResultOk(v) | Value::ResultErr(v) | Value::OptionSome(v) => {
                Some(ObjectRef::Boxed(v.0.clone()))
            }
            Value::Grounded(g) => Some(ObjectRef::Boxed(g.inner.0.clone())),
            Value::Partial(_) => None,
            Value::ResumeToken(_) => None,
            _ => None,
        }
    }
}



impl PartialEq for Value {
    fn eq(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Int(a), Value::Float(b)) => (*a as f64) == *b,
            (Value::Float(a), Value::Int(b)) => *a == (*b as f64),
            (Value::String(a), Value::String(b)) => a == b,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Nothing, Value::Nothing) => true,
            (Value::Struct(a), Value::Struct(b)) => a == b,
            (Value::List(a), Value::List(b)) => a == b,
            (Value::Weak(a), Value::Weak(b)) => a.ptr_eq(b),
            (Value::ResultOk(a), Value::ResultOk(b)) => a == b,
            (Value::ResultErr(a), Value::ResultErr(b)) => a == b,
            (Value::OptionSome(a), Value::OptionSome(b)) => a == b,
            (Value::OptionNone, Value::OptionNone) => true,
            (Value::Grounded(a), Value::Grounded(b)) => a.inner == b.inner,
            (Value::Partial(a), Value::Partial(b)) => a == b,
            (Value::ResumeToken(a), Value::ResumeToken(b)) => a == b,
            (Value::Stream(a), Value::Stream(b)) => a == b,
            _ => false,
        }
    }
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

    fn backpressure_label(&self) -> String {
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



#[cfg(test)]
mod tests {
    use super::{StructValue, Value};
    use corvid_resolve::DefId;
    use std::sync::Arc;

    #[test]
    fn struct_handle_refcount_tracks_value_clones() {
        let value = Value::Struct(StructValue::new(
            DefId(1),
            "Node",
            [("label".to_string(), Value::String(Arc::from("root")))],
        ));
        let strong = match &value {
            Value::Struct(s) => s.strong_count_for_tests(),
            _ => unreachable!(),
        };
        assert_eq!(strong, 1);

        let mut clones = Vec::new();
        for _ in 0..1000 {
            clones.push(value.clone());
        }

        let strong = match &value {
            Value::Struct(s) => s.strong_count_for_tests(),
            _ => unreachable!(),
        };
        assert_eq!(strong, 1001);

        drop(clones);

        let strong = match &value {
            Value::Struct(s) => s.strong_count_for_tests(),
            _ => unreachable!(),
        };
        assert_eq!(strong, 1);
    }
}
