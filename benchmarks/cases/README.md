# Canonical AI Benchmark Fixtures

These fixtures define the canonical workload contract for Corvid's AI workflow benchmark suite.

Use this directory for:

- the shared machine-readable workload format
- the fixed mocked inputs and outputs
- the deterministic external latency schedule
- the expected final output and replay trace shape

Every implementation in the suite must consume the same fixture semantics. No implementation may quietly substitute a different workflow while keeping the same benchmark name.

## Files

- `schema.json` — JSON Schema for benchmark fixtures
- `tool_loop.json` — repeated prompt/tool orchestration
- `retry_workflow.json` — deterministic retry and backoff
- `approval_workflow.json` — approval-gated tool execution
- `replay_trace.json` — recorded deterministic replay session

## Measurement Contract

Each step carries `external_latency_ms`. Benchmark runners must:

1. execute the workflow exactly in the listed order
2. sum all `external_latency_ms` values into `external_wait_time_ms`
3. report:

```text
orchestration_overhead_ms = total_wall_time_ms - external_wait_time_ms
```

Backoff sleeps belong in `external_wait_time_ms` and must not be counted as orchestration overhead.

## Step Semantics

Supported `kind` values:

- `prompt` — mocked model / LLM boundary
- `tool` — mocked tool / FFI boundary
- `approval` — deterministic human-in-the-loop decision
- `retry_sleep` — deterministic backoff delay
- `replay_checkpoint` — expected replay inspection boundary

Each step records:

- `name`
- `kind`
- `inputs`
- `mock_response`
- `mock_output`
- `external_latency_ms`

`mock_output` is the semantic result the runner must surface to the next step. For `prompt`, it can include:

- plain text
- structured JSON
- tool intent / selected action

For `approval`, it should include a deterministic decision such as:

- `approve`
- `deny`

For `prompt`, `mock_response` is mandatory and must be the exact raw mocked body the runner receives from the model boundary before any local parsing or interpretation.

## Determinism Rules

Each fixture includes `random_seed`.

Runners must use that seed for any operation that would otherwise introduce run-to-run variance, including:

- retry jitter
- UUID generation for trace IDs
- randomized sampling or shuffle defaults in competitor frameworks

If a stack requires an override flag or environment variable to pin its seed, that override mechanism must be documented in the runner README.

## Replay Contract

Fixtures may include `expected_replay_events`. Those are the canonical event labels the replay-capable implementations must emit in order.

If a competitor cannot support real replay:

- it may still run the functional workflow
- it must report `replay_supported = false`
- it must not claim parity with Corvid's replay step metrics

Yes: the replay format is intentionally reusable across all four workload families. A `tool_loop`, `retry_workflow`, or `approval_workflow` execution can be recorded as the same ordered step stream and replayed under the `replay_trace` contract. Replay is meant to be a first-class primitive, not a separate benchmark-only format.

## Approval Determinism

Approval steps must carry:

- `approval_outcome`: `granted` or `denied`
- `on_denied` when the outcome is `denied`

The allowed denied-path behaviors are:

- `abort_workflow`
- `skip_tool_call`

No runner may invent its own denial semantics for a shared fixture.

## Trace Size Reporting

Runners must report all three:

- `trace_size_raw_bytes`
- `logical_steps_recorded`
- `bytes_per_step`

Derived metric:

```text
bytes_per_step = trace_size_raw_bytes / logical_steps_recorded
```

`bytes_per_step` is the publishable cross-format comparison number. Raw bytes alone are not enough when one stack emits JSON and another emits a binary or compressed format.

## Allocation Reporting

Cross-stack allocation reporting is intentionally narrow:

- if a runner can report host-heap allocations honestly, do that
- if it cannot, report a qualitative note instead of fake precision

Corvid may additionally publish richer self-comparison counters such as RC ops or verifier counters, but those must stay in a Corvid-only appendix rather than the main cross-stack comparison table.

## Fairness Rules

- Do not replace the mocked responses with "equivalent" alternatives.
- Do not remove steps that are awkward for one stack.
- Do not change the latency profile between implementations.
- Do not benchmark with tracing off on one side and on for another.
- If a stack cannot support a fixture feature honestly, document the limitation in the result row.
