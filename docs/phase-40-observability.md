# Phase 40 Observability, Evals, And Production Monitoring

This document is the implementation contract for Phase 40. The goal is that a
maintainer can answer, from committed Corvid tooling, what an AI backend did,
why it did it, what it cost, who approved it, what data it touched, which
contract applied, and how to replay or promote the run into an eval.

## Non-Scope

Phase 40 does not claim to replace every hosted observability product. It ships
local-first trace storage, deterministic exports, OpenTelemetry mapping, eval
promotion, drift reports, and review queues. Hosted dashboards, multi-tenant SaaS
storage, alert paging, and vendor-specific LangSmith/Langfuse importers are
post-phase work unless a later slice explicitly adds them.

AI-assisted commands are assistive only. `corvid observe explain` may summarize
root-cause candidates from traces and docs, but the signed trace, typed
contracts, and deterministic reports remain the source of truth.

## Lineage Trace Model

Every observable event is represented as a span-like record with stable IDs:

- `trace_id`: one backend request, scheduled job run, CLI run, or replay run.
- `span_id`: stable ID for the event node.
- `parent_span_id`: empty only for the root node.
- `kind`: `route`, `job`, `agent`, `prompt`, `tool`, `approval`, `db`,
  `retry`, `error`, `eval`, or `review`.
- `name`: route path, job name, agent name, prompt name, tool name, approval
  contract ID, table/query label, or eval fixture ID.
- `status`: `ok`, `failed`, `denied`, `pending_review`, `replayed`, or
  `redacted`.
- `started_ms` / `ended_ms`: UTC epoch milliseconds.
- `tenant_id`, `actor_id`, `request_id`, `replay_key`, `idempotency_key`.
- `guarantee_id`: populated when the event is tied to a registered guarantee.
- `effect_ids`: declared effect row IDs involved in this span.
- `approval_id`: populated for approval spans and the tool/action spans they
  authorize.
- `data_classes`: normalized data classes touched by the span.
- `cost_usd`, `tokens_in`, `tokens_out`, `latency_ms`.
- `model_id`, `model_fingerprint`, `prompt_hash`, `retrieval_index_hash`.
- `input_fingerprint`, `output_fingerprint`, `redaction_policy_hash`.

Lineage completeness means every route/job/agent/prompt/tool/approval/DB row in
a run can be reached by following `parent_span_id` from the root `trace_id`.
Missing parent links are observability errors, not warnings.

## Export Format

The local export format is JSONL, one event per line:

```json
{"schema":"corvid.trace.lineage.v1","trace_id":"trace-1","span_id":"span-route","parent_span_id":"","kind":"route","name":"POST /actions/follow-up/send","status":"ok","tenant_id":"tenant-1","actor_id":"user-1","replay_key":"route:trace-1","guarantee_id":"approval.reachable_entrypoints_require_contract","effect_ids":["send_email"],"approval_id":"approval:thread-1","data_classes":["private"],"cost_usd":0.02,"latency_ms":24}
```

The format is append-only. Later schemas must add fields or new event kinds, not
reinterpret existing field meanings.

## Metrics Taxonomy

Metric names use the `corvid.` prefix:

- `corvid.request.count`, `corvid.request.duration_ms`,
  `corvid.request.error.count`.
- `corvid.job.count`, `corvid.job.retry.count`, `corvid.job.dead_letter.count`.
- `corvid.llm.call.count`, `corvid.llm.tokens`, `corvid.llm.cost_usd`,
  `corvid.llm.schema_failure.count`, `corvid.llm.confidence`.
- `corvid.tool.call.count`, `corvid.tool.error.count`,
  `corvid.tool.cost_usd`.
- `corvid.approval.created.count`, `corvid.approval.approved.count`,
  `corvid.approval.denied.count`, `corvid.approval.expired.count`,
  `corvid.approval.latency_ms`.
- `corvid.db.query.count`, `corvid.db.query.duration_ms`,
  `corvid.db.migration.drift.count`.
- `corvid.guarantee.violation.count`, labelled by `guarantee_id`.
- `corvid.replay.count`, labelled by `status`.

Required attributes: `service.name`, `corvid.trace_id`, `corvid.span_id`,
`corvid.kind`, `corvid.replay_key`, `corvid.tenant_id`, `corvid.actor_id`,
`corvid.guarantee_id`, `corvid.effect_ids`, `corvid.approval_id`, and
`corvid.data_classes`. High-cardinality raw inputs are never metric labels.

## OpenTelemetry Mapping

Corvid lineage events map to OTel spans using these semantic attributes:

- `corvid.trace_id`, `corvid.span_id`, `corvid.parent_span_id`.
- `corvid.kind`, `corvid.name`, `corvid.status`.
- `corvid.guarantee_id`, `corvid.effect_ids`, `corvid.approval_id`.
- `corvid.cost_usd`, `corvid.tokens_in`, `corvid.tokens_out`,
  `corvid.latency_ms`.
- `corvid.replay_key`, `corvid.idempotency_key`.
- `corvid.model_id`, `corvid.model_fingerprint`, `corvid.prompt_hash`,
  `corvid.retrieval_index_hash`.

OTel export may drop local-only fields only when the JSONL export keeps them.
The JSONL export is the lossless contract; OTel is the interoperability layer.

## Eval Promotion

`corvid eval promote <trace-id>` turns selected lineage spans into a fixture:

- Inputs and outputs are redacted before writing.
- The fixture stores `trace_id`, selected `span_id`s, source hashes,
  redaction policy hash, model/prompt/index fingerprints, expected output
  schema, expected guarantees, and replay keys.
- The fixture is deterministic: the same trace and redaction policy produce the
  same bytes.
- Signed promotion records are post-processed with the existing attestation
  machinery when the slice adds signing.

Promotion fails closed when required lineage is incomplete, raw secrets remain
after redaction, or an expected guarantee ID is not registered.

## Redaction

Redaction policies are explicit and hashed. Built-in redactors cover email,
phone, name, SSN-like IDs, bearer/API tokens, OAuth state, session IDs, and raw
tool arguments marked sensitive. Replacement tokens are deterministic per
policy:

```text
<redacted:email:sha256:8f2a...>
```

No promoted eval, observe report, or OTel attribute may contain raw secrets.
Adversarial tests must include fake SSNs and tokens and assert zero raw matches
in promoted fixtures.

## Retention

Default local retention:

- Raw local traces: 7 days.
- Redacted lineage summaries: 90 days.
- Promoted eval fixtures: until removed by source control.
- OTel export buffers: best effort, flushed before shutdown.

Retention must be configurable, but shorter retention must not delete promoted
fixtures or signed evidence without an explicit command.

## Drift And Regression Reports

Drift reports compare two trace sets or a trace against a promoted eval:

- output schema failures
- confidence drops
- cost changes
- latency changes
- approval denial spikes
- tool-error spikes
- model fingerprint changes
- prompt hash changes
- retrieval index hash changes

Reports group by guarantee, effect, approval rule, prompt, model, tool, and
route/job entrypoint. CI output must be stable text plus JSON.

## Human Review Queues

Review queue records link back to lineage:

- `review_id`, `trace_id`, `span_id`, `tenant_id`, `actor_id`.
- `reason`: low confidence, high risk, denied approval, schema failure,
  guarantee violation, or operator escalation.
- `cost_of_being_wrong`: numeric rank input.
- `source_prompt_hash`, `model_fingerprint`, `approval_id`, `replay_key`.
- terminal decision with reviewer actor and audit event.

The review queue is not a chat product. It is a typed backend queue with trace
and audit linkage.

## Operator Questions

Phase 40 tooling must answer:

- What happened? Render the lineage tree by `trace_id`.
- Why did it happen? Show prompt/model/tool/approval/guarantee nodes on the
  path.
- What did it cost? Sum `cost_usd` by trace, route, job, agent, model, tool,
  effect, and guarantee.
- Who approved it? Join approval spans and audit envelopes by `approval_id`.
- What data did it touch? Show `data_classes`, provenance, and retrieval index
  hashes.
- Can I replay it? Show `replay_key`, required fixtures, and missing evidence.

These questions become the acceptance criteria for the later implementation
slices.
