# Phase 40 Operator Runbook

This runbook maps production operator questions to committed Corvid commands and
runtime artifacts. Lineage JSONL remains the lossless source of truth; rendered
reports, OTel payloads, drift reports, review queues, and promoted eval fixtures
are derived evidence.

## Trace Store

Default local trace directory:

```sh
target/trace
```

Phase 40 lineage files use:

```sh
<trace-id>.lineage.jsonl
```

Each line is a `corvid.trace.lineage.v1` event.

## What Happened?

List local production runs:

```sh
corvid observe list --trace-dir target/trace
```

Render one lineage tree:

```sh
corvid trace lineage <trace-id> --trace-dir target/trace
```

Show one run with contract-aware grouping:

```sh
corvid observe show <trace-id> --trace-dir target/trace
```

## Why Did It Happen?

Use the contract-aware incident section from:

```sh
corvid observe show <trace-id> --trace-dir target/trace
```

The report groups failures by guarantee, effect, budget, provenance, and
approval rule. The same grouping is available programmatically through
`corvid_runtime::group_lineage_incidents`.

## What Did It Cost?

List all local runs with per-run cost:

```sh
corvid observe list --trace-dir target/trace
```

Compare baseline and candidate trace sets for cost drift:

```sh
corvid observe drift target/trace/baseline target/trace/candidate
```

CI JSON form:

```sh
corvid observe drift target/trace/baseline target/trace/candidate --json
```

## Who Approved It?

Show approval IDs and approval incident groups:

```sh
corvid observe show <trace-id> --trace-dir target/trace
```

Runtime review records preserve `approval_id`, `audit_event_id`,
`reviewer_actor_id`, `trace_id`, and `span_id` through
`corvid_runtime::ReviewQueueRuntime`.

## What Data Did It Touch?

Show data-class groups:

```sh
corvid observe show <trace-id> --trace-dir target/trace
```

Promoted fixtures and downstream reports should use redacted lineage:

```sh
corvid eval promote target/trace/<trace-id>.lineage.jsonl --promote-out target/eval/lineage
```

## Can I Replay It?

Find replay keys in the run:

```sh
corvid observe show <trace-id> --trace-dir target/trace
```

Replay legacy recorded execution traces:

```sh
corvid replay target/trace/<run-id>.jsonl --source path/to/source.cor
```

Promote a lineage trace into regression evidence:

```sh
corvid eval promote target/trace/<trace-id>.lineage.jsonl --promote-out target/eval/lineage
```

## Did A Change Regress Production Behavior?

Human-readable drift report:

```sh
corvid observe drift target/trace/baseline target/trace/candidate
```

CI-friendly JSON:

```sh
corvid observe drift target/trace/baseline target/trace/candidate --json
```

The command exits non-zero when schema violations, denials, tool errors, cost,
latency, or confidence regress.

## How Do I Export To OpenTelemetry?

Use the runtime exporter:

```rust
use corvid_runtime::{build_otel_export_batch, OtelExporterConfig, OtelHttpExporter};
```

Payload-only validation:

```rust
let config = OtelExporterConfig::local("my-corvid-service");
let batch = build_otel_export_batch(&events, &config)?;
```

OTLP/HTTP delivery:

```rust
let exporter = OtelHttpExporter::new(config)?;
let report = exporter.export_lineage(&events).await?;
```

## How Do I Queue Human Review?

Runtime queue:

```rust
use corvid_runtime::{ReviewQueuePolicy, ReviewQueueRuntime};

let mut queue = ReviewQueueRuntime::new();
let policy = ReviewQueuePolicy::default();
let maybe_review = queue.enqueue_if_required(&event, cost_of_being_wrong, &policy, now_ms)?;
```

Resolve with audit evidence:

```rust
queue.resolve(
    review_id,
    ReviewStatus::Approved,
    "reviewer-actor-id",
    "audit-event-id",
    "decision note",
    now_ms,
)?;
```

## Phase 40 Acceptance Check

Before declaring an application observable, verify:

- `corvid observe list` shows all expected lineage runs.
- `corvid observe show <id>` includes approval IDs, replay keys, data classes,
  and incident groups.
- `corvid observe drift ... --json` runs in CI.
- `corvid eval promote ...` writes redacted deterministic fixtures.
- OTel export batches contain spans and metrics for requests, jobs, LLM calls,
  tools, approvals, errors, retries, costs, and replay IDs.
