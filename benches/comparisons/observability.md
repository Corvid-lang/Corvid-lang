# Phase 40 — observability + evals side-by-side

## Headline

For an idiomatic incident-diagnosis flow ("what did the agent do,
why, what did it cost, who approved it, what data did it touch, and
can I replay it?"), Corvid's typed lineage + signed eval promotion
+ contract-aware grouping answers all six questions from one
committed CLI surface. The OpenTelemetry + LangSmith / Langfuse
baselines answer cost / latency / spans, but cost-by-guarantee,
approval-by-trace, and eval-promotion-with-redacted-lineage require
custom queries on top.

## Reproduce

The Corvid implementation lives in
`crates/corvid-runtime/src/{lineage,lineage_drift,lineage_eval,lineage_incidents,lineage_redact,lineage_render,otel_export,otel_schema,review_queue}.rs`
and the CLI subcommands in
`crates/corvid-cli/src/{observe_cmd,eval_cmd}.rs`.

After audit-correction slice 40J the OTel export uses the standard
`opentelemetry` SDK and the docker-compose Jaeger conformance test
exercises the full path; until then the export uses hand-rolled
JSON over `reqwest` (functional, but the conformance test cannot
run).

## Side-by-side (sketch)

### Corvid

```bash
corvid observe show <trace-id>           # lineage tree + cost + approvals + guarantees
corvid observe explain <trace-id>        # AI-assisted root cause (RAG-grounded over the typed trace)
corvid observe cost --by=guarantee_id
corvid observe drift --from=<id> --to=<id>
corvid eval promote <trace-id> --redact=email,phone,name
corvid eval drift --explain              # decompose model / input / prompt / index drift
corvid review-queue list --rank=cost-of-being-wrong
corvid observe export --otlp=https://otel.host:4317
```

Lineage IDs (`trace_id`, parent `span_id`) live on every route /
job / agent / prompt / tool / approval / DB row in the schema, so
the queries above are SQL `JOIN`s against the trace store. Spans
carry `corvid.guarantee_id`, `corvid.cost_usd`,
`corvid.approval_id`, `corvid.replay_key` attributes (registry rows
`observability.lineage_completeness`, `observability.otel_conformance`,
`eval.drift_attribution`, `eval.promotion_signed_lineage`).

### Python (LangSmith + OpenTelemetry) — bounty-open

LangSmith ships traces, latency, and spans; cost and approval
linkage require manual association. OpenTelemetry ships standard
spans without `corvid.guarantee_id`-shaped attributes; an incident
analyst groups by service.name, not by violated guarantee.
Eval promotion typically means writing a fresh LangChain test from
a trace by hand. Submission lands under `runs/python/`.

### TypeScript (Langfuse + OpenTelemetry) — bounty-open

Langfuse ships a trace explorer with cost and prompt metadata;
similar limitations to LangSmith on contract-aware grouping. The
eval-promote workflow is a Langfuse "dataset" upload from a trace
sample. Submission lands under `runs/typescript/`.

## Time-to-answer (sketch)

| Question | Corvid | LangSmith / Langfuse + OTel | Notes |
|---|---|---|---|
| What did the agent do? | `corvid observe show <id>` (1 command) | trace explorer (UI) | both have it |
| Why? (LLM rationale + grounded sources) | typed `Grounded<T>` in the trace | manual prompt-output reconstruction | Corvid's typed surface |
| What did it cost? | `corvid observe cost --by=guarantee_id` | LangSmith cost dashboard | both, but Corvid groups by guarantee |
| Who approved it? | `corvid.approval_id` attribute on the span | application-layer logs | Corvid native |
| What data did it touch? | `data:` effect dimension on the trace | application-layer logs | Corvid native |
| Can I replay it? | `corvid.replay_key` + replay quarantine | re-run the workflow with logging | Corvid prevents double side-effects |

## What Corvid wins on

- **Contract-aware grouping**: incidents group by guarantee /
  effect / budget / provenance / approval rule, not by
  `service.name`. Registry row
  `observability.contract_aware_grouping` covers this.
- **Signed eval promotion**: `corvid eval promote` writes a
  fixture whose lineage is signed by the same ed25519 key that
  signed the cdylib. `eval.promotion_signed_lineage` makes the
  claim concrete.
- **Replay quarantine** prevents `corvid observe replay <id>`
  from issuing real provider calls.

## What Corvid does not claim

- **Trace UI ergonomics** are not yet on par with LangSmith /
  Langfuse; `corvid observe` is CLI-first.
- **Drift attribution** uses synthetic swaps for the
  model / prompt / index dimensions in tests; production drift
  attribution depends on the eval corpus the project maintains.
- **Time-to-answer numbers above are descriptive, not measured.**
  The `bounty-open` cells fill in once an idiomatic LangSmith /
  Langfuse implementation lands as a submission.
