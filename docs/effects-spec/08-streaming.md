# 08 — Streaming effects

The headline claim: **every dimensional property holds mid-stream, not just at completion.**

Standard LLM clients expose streaming as a "give me tokens as they arrive" API and leave safety to the caller. Corvid treats streaming as a first-class language construct: `Stream<T>` is a type, `yield` is a statement, and the effect system's cost/confidence/token bounds all *terminate the stream* the moment they'd be exceeded.

## 1. `Stream<T>` as a return type

```corvid
# expect: skip
tool fetch_doc(id: String) -> String

prompt generate(context: String) -> Stream<String>:
    with max_tokens 500
    "Generate from {context}"

agent stream_research(id: String) -> Stream<String>:
    context = fetch_doc(id)
    for chunk in generate(context):
        yield chunk
```

The agent's return type is `Stream<String>`. The body uses `yield` instead of `return` to emit values as they become available. The caller iterates:

```corvid
# expect: skip
agent caller(id: String) -> Nothing:
    for chunk in stream_research(id):
        print(chunk)
```

`for chunk in <stream>` is the standard consumption form — see the `lower.rs` IR handling.

## 2. Yield vs. return

`yield` emits a value into the stream and continues. `return` terminates the stream (optionally with a final value). An agent's body may use `yield` zero or many times, and may end with `return` or implicit end-of-body.

The checker rejects mismatches:

- `yield` in an agent that declares a non-`Stream<T>` return type → `YieldRequiresStreamReturn`.
- An agent declaring `Stream<T>` but never yielding → `StreamReturnWithoutYield` (warning, not error — the stream just produces no values).
- `yield <value>` whose value type doesn't match the stream's element type → `YieldReturnTypeMismatch`.

## 3. Backpressure

Streams decouple production rate from consumption rate. Corvid's streaming runtime buffers between producer and consumer via `tokio::mpsc`, and the dimensional system captures the buffering policy:

```corvid
# expect: skip
prompt fast_stream(q: String) -> Stream<String>:
    with backpressure bounded(100)
    "..."
```

Options:
- `bounded(N)` — the buffer holds up to N items; when full, the producer blocks until the consumer catches up.
- `unbounded` — the buffer grows without bound (dangerous for long-running streams; use deliberately).

The `backpressure` clause sets the `latency` dimension to `streaming(backpressure: ...)`. See [02 composition-algebra](./02-composition-algebra.md) for how streaming latencies compose.

## 4. Mid-stream termination

The runtime enforces `@budget` and `@min_confidence` **live**, not at agent completion:

- **Cost.** After each emitted token, cumulative cost updates. Crossing the `@budget($N)` bound terminates the stream and raises `BudgetExceeded`.
- **Tokens.** Same mechanism over `tokens` budget dimension.
- **Latency.** Same mechanism over `latency_ms` budget dimension.
- **Confidence.** If the prompt emits confidence alongside each chunk and cumulative min crosses `@min_confidence(C)` from above, the stream terminates with `ConfidenceViolation`.

No other language's effect system enforces budgets mid-stream. Most runtimes would let a streaming prompt run to completion and then report the violation after the fact — by which time the money has been spent and the tokens consumed.

## 5. `try ... retry` on streams (start-of-stream semantics)

Corvid's `retry` construct wraps a stream expression. The retry policy only applies *before* the first emitted value:

```corvid
# expect: skip
for chunk in try generate(context) retry attempts 3 backoff exponential:
    process(chunk)
```

Once the stream has emitted its first value, retry becomes a no-op — partial stream data can't be retried without replaying the already-consumed prefix. This is deliberate. Callers that want full replay must model their stream as a series of retryable blocks, not a single retryable stream.

## 6. Progressive structured types: `Stream<Partial<T>>`

Research-stage, not yet shipped. The idea: a stream of `Partial<T>` values where each emission represents the latest complete-enough approximation. The type system can know that certain fields are filled before others, enabling "display the user ID as soon as it arrives, even if the rest of the response is still streaming."

Roadmap: `Stream<Partial<T>>` with per-field `Complete(V) | Streaming` markers, compile-time detection of which fields are available at each stage. ROADMAP Phase 20f captures the design.

## 7. Resumption tokens (planned)

Cancellation produces a typed `resume_token`. A future `resume(prompt, token)` call continues from the interruption point — either via provider-native continuation APIs or by replaying with the accumulated context. Planned for Phase 20f's streaming slice.

## 8. Declarative fan-out / fan-in (planned)

`stream.split_by(key)` partitions one stream into sub-streams by an extractor. `merge(streams) ordered_by(policy)` combines multiple streams with a deterministic ordering (FIFO, sorted, fair round-robin). Both compile-time typed and effect-checked. Roadmap.

## 9. Implementation references

- AST: `Type::Stream`, `Stmt::Yield`, `BackpressurePolicy` in [../../crates/corvid-ast/](../../crates/corvid-ast/).
- IR: `IrStmt::Yield`, `IrPrompt::backpressure` in [../../crates/corvid-ir/src/types.rs](../../crates/corvid-ir/src/types.rs).
- Runtime: `tokio::mpsc`-backed stream runtime in [../../crates/corvid-vm/src/interp.rs](../../crates/corvid-vm/src/interp.rs).
- Live cost termination: search for `BudgetExceeded` in the same file.

## Next

[09 — Typed model substrate](./09-model-substrate.md) — the Phase 20h preview: `model` declarations, capability routing, content-aware dispatch, cost-frontier visualization.
