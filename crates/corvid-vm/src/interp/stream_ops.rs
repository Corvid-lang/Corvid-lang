use crate::conv::value_to_json;
use crate::errors::{InterpError, InterpErrorKind};
use crate::value::{ListValue, StreamChunk, StreamValue, Value};
use corvid_ast::{BackpressurePolicy, Span};
use corvid_ir::StreamMergePolicy;
use std::collections::BTreeMap;

pub(super) async fn split_by(
    value: Value,
    key: &str,
    span: Span,
) -> Result<Value, InterpError> {
    let Value::Stream(stream) = value else {
        return Err(type_mismatch("Stream<T>", value.type_name(), span));
    };

    let backpressure = stream.backpressure().clone();
    let mut order = Vec::new();
    let mut groups: BTreeMap<String, Vec<StreamChunk>> = BTreeMap::new();
    while let Some(item) = stream.next_chunk().await {
        let chunk = item?;
        let group_key = group_key(&chunk.value, key, span)?;
        if !groups.contains_key(&group_key) {
            order.push(group_key.clone());
        }
        groups.entry(group_key).or_default().push(chunk);
    }

    let mut out = Vec::with_capacity(order.len());
    for group_key in order {
        let chunks = groups.remove(&group_key).unwrap_or_default();
        out.push(Value::Stream(stream_from_chunks(chunks, backpressure.clone()).await));
    }
    Ok(Value::List(ListValue::new(out)))
}

pub(super) async fn merge(
    value: Value,
    policy: StreamMergePolicy,
    span: Span,
) -> Result<Value, InterpError> {
    let (streams, backpressure) = streams_from_value(value, span)?;
    let chunks = match policy {
        StreamMergePolicy::Fifo => collect_fifo(streams).await?,
        StreamMergePolicy::FairRoundRobin => collect_fair_round_robin(streams).await?,
        StreamMergePolicy::Sorted => collect_sorted(streams).await?,
    };
    Ok(Value::Stream(stream_from_chunks(chunks, backpressure).await))
}

pub(super) async fn ordered_by(
    value: Value,
    policy: StreamMergePolicy,
    span: Span,
) -> Result<Value, InterpError> {
    let Value::Stream(stream) = value else {
        return Err(type_mismatch("Stream<T>", value.type_name(), span));
    };
    match policy {
        StreamMergePolicy::Fifo | StreamMergePolicy::FairRoundRobin => Ok(Value::Stream(stream)),
        StreamMergePolicy::Sorted => {
            let backpressure = stream.backpressure().clone();
            let mut chunks = collect_stream(stream).await?;
            sort_chunks(&mut chunks);
            Ok(Value::Stream(stream_from_chunks(chunks, backpressure).await))
        }
    }
}

async fn collect_fifo(streams: Vec<StreamValue>) -> Result<Vec<StreamChunk>, InterpError> {
    let mut chunks = Vec::new();
    for stream in streams {
        chunks.extend(collect_stream(stream).await?);
    }
    Ok(chunks)
}

async fn collect_fair_round_robin(
    streams: Vec<StreamValue>,
) -> Result<Vec<StreamChunk>, InterpError> {
    let mut active = streams.into_iter().map(Some).collect::<Vec<_>>();
    let mut remaining = active.len();
    let mut chunks = Vec::new();
    while remaining > 0 {
        for slot in &mut active {
            let Some(stream) = slot else {
                continue;
            };
            match stream.next_chunk().await {
                Some(item) => chunks.push(item?),
                None => {
                    *slot = None;
                    remaining -= 1;
                }
            }
        }
    }
    Ok(chunks)
}

async fn collect_sorted(streams: Vec<StreamValue>) -> Result<Vec<StreamChunk>, InterpError> {
    let mut chunks = collect_fifo(streams).await?;
    sort_chunks(&mut chunks);
    Ok(chunks)
}

async fn collect_stream(stream: StreamValue) -> Result<Vec<StreamChunk>, InterpError> {
    let mut chunks = Vec::new();
    while let Some(item) = stream.next_chunk().await {
        chunks.push(item?);
    }
    Ok(chunks)
}

async fn stream_from_chunks(
    chunks: Vec<StreamChunk>,
    backpressure: BackpressurePolicy,
) -> StreamValue {
    let (sender, stream) = StreamValue::channel(backpressure);
    for chunk in chunks {
        if !sender.send_chunk(Ok(chunk)).await {
            break;
        }
    }
    drop(sender);
    stream
}

fn streams_from_value(
    value: Value,
    span: Span,
) -> Result<(Vec<StreamValue>, BackpressurePolicy), InterpError> {
    let Value::List(items) = value else {
        return Err(type_mismatch("List<Stream<T>>", value.type_name(), span));
    };
    let mut out = Vec::new();
    let mut backpressure = None;
    for item in items.iter_cloned() {
        match item {
            Value::Stream(stream) => {
                backpressure = Some(match backpressure {
                    Some(policy) => compose_backpressure(&policy, stream.backpressure()),
                    None => stream.backpressure().clone(),
                });
                out.push(stream);
            }
            other => return Err(type_mismatch("Stream<T>", other.type_name(), span)),
        }
    }
    Ok((out, backpressure.unwrap_or(BackpressurePolicy::Bounded(1))))
}

fn compose_backpressure(
    current: &BackpressurePolicy,
    incoming: &BackpressurePolicy,
) -> BackpressurePolicy {
    match (current, incoming) {
        (BackpressurePolicy::Unbounded, _) | (_, BackpressurePolicy::Unbounded) => {
            BackpressurePolicy::Unbounded
        }
        (BackpressurePolicy::PullsFrom(a), BackpressurePolicy::PullsFrom(b)) if a == b => {
            BackpressurePolicy::PullsFrom(a.clone())
        }
        (BackpressurePolicy::PullsFrom(_), BackpressurePolicy::PullsFrom(_)) => {
            BackpressurePolicy::Bounded(1)
        }
        (BackpressurePolicy::PullsFrom(_), BackpressurePolicy::Bounded(size))
        | (BackpressurePolicy::Bounded(size), BackpressurePolicy::PullsFrom(_)) => {
            BackpressurePolicy::Bounded(*size)
        }
        (BackpressurePolicy::Bounded(a), BackpressurePolicy::Bounded(b)) => {
            BackpressurePolicy::Bounded((*a).max(*b))
        }
    }
}

fn group_key(value: &Value, key: &str, span: Span) -> Result<String, InterpError> {
    let Value::Struct(struct_value) = value else {
        return Err(type_mismatch("struct stream element", value.type_name(), span));
    };
    let field = struct_value.get_field(key).ok_or_else(|| {
        InterpError::new(
            InterpErrorKind::UnknownField {
                struct_name: struct_value.type_name().to_string(),
                field: key.to_string(),
            },
            span,
        )
    })?;
    Ok(match field {
        Value::String(value) => value.to_string(),
        Value::Int(value) => value.to_string(),
        Value::Float(value) => value.to_string(),
        Value::Bool(value) => value.to_string(),
        other => value_to_json(&other).to_string(),
    })
}

fn sort_chunks(chunks: &mut [StreamChunk]) {
    chunks.sort_by(|left, right| {
        value_to_json(&left.value)
            .to_string()
            .cmp(&value_to_json(&right.value).to_string())
    });
}

fn type_mismatch(expected: impl Into<String>, got: impl Into<String>, span: Span) -> InterpError {
    InterpError::new(
        InterpErrorKind::TypeMismatch {
            expected: expected.into(),
            got: got.into(),
        },
        span,
    )
}
