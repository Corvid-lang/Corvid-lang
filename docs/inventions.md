# Corvid Inventions

Corvid is a general-purpose language with AI built into the compiler. These are
the shipped language ideas that make it different from Python libraries,
TypeScript frameworks, and ordinary model SDKs.

This page is intentionally independent of build instructions. It answers one
question: what can Corvid express as a language that other ecosystems usually
express as runtime glue?

## 1. Safety At Compile Time

### Approve Before Dangerous

```corvid
tool issue_refund(id: String) -> Receipt dangerous

agent refund(id: String) -> Receipt:
    approve IssueRefund(id)
    return issue_refund(id)
```

Corvid makes irreversible authority visible. A dangerous tool call without a
prior `approve` boundary is rejected by the compiler.

Why it is unique: ordinary languages can only ask a library to remember whether
approval happened. Corvid makes the boundary part of the program's static
contract.

### Dimensional Effects

```corvid
effect llm_call:
    cost: $0.05
    trust: autonomous
    reversible: true

prompt summarize(text: String) -> String uses llm_call:
    "Summarize {text}"
```

Effects in Corvid are structured dimensions, not flat labels. Cost, trust,
reversibility, data, latency, confidence, and custom dimensions compose through
declared algebra.

Why it is unique: AI applications carry money, trust, privacy, reversibility,
and confidence through the same workflow. Corvid lets the compiler reason about
those dimensions together.

### Grounded<T>

```corvid
effect retrieval:
    data: grounded

tool fetch_doc(id: String) -> Grounded<String> uses retrieval

agent answer(id: String) -> Grounded<String>:
    return fetch_doc(id)
```

`Grounded<T>` means a value must be connected to retrieval provenance. The
compiler rejects grounded returns that have no grounded source.

Why it is unique: grounding is usually a prompt convention or a RAG library
habit. Corvid makes it a type.

### Strict Citations

```corvid
prompt answer(ctx: Grounded<String>) -> Grounded<String>:
    cites ctx strictly
    "Answer from {ctx}"
```

A prompt can require citations to a specific grounded parameter. The compiler
checks the parameter's grounded type; runtime checks the model response.

Why it is unique: the citation requirement is not just text inside the prompt.
It is a contract the compiler and runtime both understand.

### Compile-Time Budgets

```corvid
effect cheap_call:
    cost: $0.05

@budget($0.10)
agent bounded(text: String) -> String:
    first = classify(text)
    return classify(first)
```

Corvid can reject workflows whose declared worst-case cost exceeds the agent's
budget.

Why it is unique: most systems discover AI cost after execution. Corvid can
make cost a static bound.

### Confidence Gates

```corvid
effect llm_decision:
    confidence: 0.95

@min_confidence(0.90)
agent bot(query: String) -> String:
    return search(query)
```

Confidence is a dimension that composes by weakest link. Agents can require a
confidence floor before autonomous action.

Why it is unique: confidence stops being a loose telemetry field and becomes a
constraint that can block unsafe autonomy.

## 2. AI-Native Ergonomics

### AI-Native Keywords

```corvid
model local:
    capability: basic

prompt say(name: String) -> String:
    requires: basic
    "Hello {name}"

agent hello(name: String) -> String:
    return say(name)
```

Corvid has syntax for agents, tools, prompts, effects, approvals, models, evals,
replay, and streams.

Why it is unique: the compiler can only protect what it can see. Corvid exposes
the AI boundaries directly in source.

### Trace-Aware Evals

```corvid
eval refund_accuracy:
    result = refund_bot(ticket)
    assert result.should_refund == true
```

Corvid eval declarations are designed to assert on behavior, including trace
events such as calls, approvals, ordering, and cost.

Why it is unique: output-only tests miss agents that get the right answer
through the wrong process. Trace-aware evals target process correctness.

### Replay And Receipts

```corvid
@deterministic
@replayable
agent classify(text: String) -> String:
    return text
```

Corvid executions can become traces, replay artifacts, diffs, signed receipts,
and verification bundles.

Why it is unique: AI behavior changes are usually invisible. Corvid turns them
into artifacts that can be audited and compared.

## 3. Adaptive Routing

### Typed Model Routing

```corvid
model fast:
    capability: basic

model deep:
    capability: expert

prompt answer(q: String) -> String:
    route:
        q == "hard" -> deep
        _ -> fast
    "Answer {q}"
```

Models are declarations with capabilities and policy dimensions. Prompt routing
is checked against those facts.

Why it is unique: model selection becomes a typed program decision instead of a
string hidden in runtime glue.

### Progressive Refinement

```corvid
prompt classify(q: String) -> String:
    progressive:
        cheap below 0.80
        medium below 0.95
        expensive
    "Classify {q}"
```

Prompts can try cheaper models first and escalate only when confidence is not
high enough.

Why it is unique: cost-quality tradeoffs are visible in source and can be
reviewed with the rest of the program.

### Ensemble Voting

```corvid
prompt classify(q: String) -> String:
    ensemble [opus, sonnet, haiku] vote majority
    "Classify {q}"
```

One prompt can dispatch to multiple models and fold the responses through a
typed voting strategy.

Why it is unique: consensus becomes a language-level strategy, not an
unreviewed helper function.

### Jurisdiction And Privacy Routing

```corvid
model eu_private:
    jurisdiction: eu_hosted
    compliance: gdpr
    privacy_tier: strict
    capability: expert
```

Model declarations can include privacy, compliance, and jurisdiction facts.

Why it is unique: data-placement policy can be checked before a prompt crosses
the wrong boundary.

## 4. Streaming

### Streaming Effects

```corvid
agent count() -> Stream<Int>:
    yield 1
    yield 2
```

Streams are typed values that can carry provenance, confidence, cost, and
backpressure semantics.

Why it is unique: streaming AI is usually an untyped callback path. Corvid keeps
it inside the language.

### Progressive Structured Streams

```corvid
type Plan:
    title: String
    body: String

agent read(snapshot: Partial<Plan>) -> Option<String>:
    return snapshot.title
```

`Partial<T>` exposes complete fields as they arrive while the rest of a
structured response is still forming.

Why it is unique: users can work with partial structured output safely instead
of parsing incomplete JSON.

### Typed Stream Resumption

```corvid
agent capture(topic: String) -> ResumeToken<String>:
    stream = draft(topic)
    return resume_token(stream)
```

`ResumeToken<T>` preserves the stream element contract across interruption and
continuation.

Why it is unique: resumption is typed. A token for one stream shape cannot be
used as another.

### Declarative Fan-Out / Fan-In

```corvid
agent fanout() -> Stream<Event>:
    groups = source().split_by("kind")
    return merge(groups).ordered_by("fair_round_robin")
```

Streams can split by structured fields and merge back with deterministic
ordering.

Why it is unique: stream topology is declared in the program, so the compiler
and runtime can preserve ordering and effect metadata.

## 5. Verification

### Proof-Carrying Dimension Registry

```corvid
effect local_policy:
    data: pii
    reversible: true

tool read_profile(id: String) -> String uses local_policy
```

Custom effect dimensions can be distributed as signed artifacts with law checks,
proof pointers, and regression programs.

Why it is unique: the effect system can grow without asking users to trust
arbitrary executable packages.

### Adversarial Bypass Testing

```corvid
tool refund(id: String) -> String dangerous uses transfer_money

@trust(human_required)
agent safe_refund(id: String) -> String:
    approve Refund(id)
    return refund(id)
```

Corvid includes a bypass taxonomy so the effect checker can be attacked by a
deterministic adversarial corpus.

Why it is unique: the language uses adversarial testing against its own safety
claims instead of treating them as prose.

## Proof Matrix

| Invention | Status | Runnable command | Test coverage | Spec | Explicit non-scope |
|---|---|---|---|---|---|
| Approve Before Dangerous | Shipped | `corvid tour --topic approve-gates` | `crates/corvid-types/src/lib.rs` | [`03-typing-rules.md`](./effects-spec/03-typing-rules.md) | Proves the approval boundary, not approval quality. |
| Dimensional Effects | Shipped | `corvid tour --topic dimensional-effects` | `crates/corvid-types/src/effects.rs` | [`02-composition-algebra.md`](./effects-spec/02-composition-algebra.md) | Proves declared contracts, not provider honesty. |
| Grounded<T> | Shipped | `corvid tour --topic grounded-values` | `crates/corvid-types/src/effects/grounded.rs` | [`05-grounding.md`](./effects-spec/05-grounding.md) | Proves source linkage, not source truth. |
| Strict Citations | Shipped | `corvid tour --topic strict-citations` | `crates/corvid-vm/src/tests/dispatch.rs` | [`05-grounding.md`](./effects-spec/05-grounding.md) | Checks citation evidence, not factual correctness. |
| Compile-Time Budgets | Shipped | `corvid tour --topic cost-budgets` | `crates/corvid-types/src/effects/cost.rs` | [`07-cost-budgets.md`](./effects-spec/07-cost-budgets.md) | Static declared costs, not invoice reconciliation. |
| Confidence Gates | Shipped | `corvid tour --topic confidence-gates` | `crates/corvid-types/src/tests.rs` | [`06-confidence-gates.md`](./effects-spec/06-confidence-gates.md) | Depends on calibrated adapter confidence. |
| AI-Native Keywords | Shipped | `corvid tour --topic language-keywords` | `crates/corvid-syntax/src/parser/tests.rs` | [`01-dimensional-syntax.md`](./effects-spec/01-dimensional-syntax.md) | Does not replace ordinary general-purpose code. |
| Trace-Aware Evals | Shipped | `corvid tour --topic eval-traces` | `crates/corvid-types/src/lib.rs` | [`12-verification.md`](./effects-spec/12-verification.md) | Full eval runner is later workflow tooling. |
| Replay And Receipts | Shipped | `corvid tour --topic replay-receipts` | `crates/corvid-cli/tests/bundle_verify.rs` | [`14-replay.md`](./effects-spec/14-replay.md) | Receipts are observed evidence, not full formal verification. |
| Typed Model Routing | Shipped | `corvid tour --topic model-routing` | `crates/corvid-vm/src/tests/dispatch.rs` | [`13-model-substrate-shipped.md`](./effects-spec/13-model-substrate-shipped.md) | Does not benchmark model quality automatically. |
| Progressive Refinement | Shipped | `corvid tour --topic progressive-routing` | `crates/corvid-vm/src/tests/dispatch.rs` | [`13-model-substrate-shipped.md`](./effects-spec/13-model-substrate-shipped.md#135-progressive-refinement-slice-e) | Thresholds need calibrated confidence. |
| Ensemble Voting | Shipped | `corvid tour --topic ensemble-voting` | `crates/corvid-vm/src/tests/dispatch.rs` | [`13-model-substrate-shipped.md`](./effects-spec/13-model-substrate-shipped.md#137-ensemble-voting-slice-f) | Custom vote functions need future function values. |
| Jurisdiction And Privacy Routing | Shipped | `corvid tour --topic privacy-routing` | `crates/corvid-types/src/effects.rs` | [`13-model-substrate-shipped.md`](./effects-spec/13-model-substrate-shipped.md#134-regulatory--compliance--privacy-dimensions-slice-d) | Legal compliance still needs operations and audits. |
| Streaming Effects | Shipped | `corvid tour --topic streaming-effects` | `crates/corvid-vm/src/tests/stream.rs` | [`08-streaming.md`](./effects-spec/08-streaming.md) | Provider-native continuation depends on providers. |
| Progressive Structured Streams | Shipped | `corvid tour --topic partial-streams` | `crates/corvid-types/src/tests.rs` | [`08-streaming.md`](./effects-spec/08-streaming.md) | Full native parity remains backend work. |
| Typed Stream Resumption | Shipped | `corvid tour --topic stream-resume` | `crates/corvid-vm/src/tests/stream.rs` | [`08-streaming.md`](./effects-spec/08-streaming.md) | Provider-native session continuation is future adapter work. |
| Declarative Fan-Out / Fan-In | Shipped | `corvid tour --topic stream-fanout` | `crates/corvid-types/src/tests.rs` | [`08-streaming.md`](./effects-spec/08-streaming.md) | Lambda extractors wait for function values. |
| Proof-Carrying Dimension Registry | Shipped | `corvid tour --topic effect-registry` | `crates/corvid-driver/src/dimension_registry.rs` | [`dimension-artifacts.md`](./effects-spec/dimension-artifacts.md) | Distributes declarations, not executable code. |
| Adversarial Bypass Testing | Shipped | `corvid tour --topic adversarial-tests` | `crates/corvid-driver/src/adversarial.rs` | [`adversarial-taxonomy.md`](./effects-spec/adversarial-taxonomy.md) | Live LLM generation expands but does not replace deterministic gates. |
