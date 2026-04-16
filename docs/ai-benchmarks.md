# AI Workflow Benchmarks

Status: draft benchmark specification.

This document defines the benchmark suite Corvid will use to support the claim:

> Corvid executes replay-audited, tool-calling AI workflows faster than orchestration stacks assembled from libraries.

The goal is not to prove that Corvid is "the fastest language." The goal is to measure the category Corvid is actually built for:

- AI-native workflows
- tool and approval boundaries
- deterministic replay
- runtime verification and low audit cost

## Principles

Every implementation in the suite must obey these rules:

- same workflow graph
- same mocked model responses
- same mocked tool outputs
- same retry policy
- same approval policy
- same final structured output
- same tracing / replay mode for all stacks that support it
- same hardware
- same run settings

If a competing stack cannot support true deterministic replay, that limitation must be documented explicitly. The benchmark may still include that stack, but the missing feature must not be hidden.

## Primary Claim

The headline metric is:

> orchestration overhead excluding external wait time

Why:

- model latency and real tool/network latency swamp runtime quality
- Corvid should win on the overhead around those boundaries
- this isolates compiler/runtime quality from mocked external sleep

Secondary metrics still matter:

- total wall time
- audit-on vs audit-off ratio
- trace / replay overhead
- allocations
- memory traffic proxies where available
- replay artifact size

## Competitor Set

The initial comparison set is:

- Corvid native runtime
- Python orchestration stack
- TypeScript orchestration stack

Initial competitor candidates:

- Python: `PydanticAI` or `LangGraph`
- TypeScript: `LangChain JS` or `Vercel AI SDK + orchestration glue`

Optional later comparison:

- Rust orchestration stack

The first publishable suite only needs one Python and one TypeScript stack, as long as the chosen stacks are widely recognized and the feature-match is documented honestly.

## Measurement Rules

All implementations must produce one machine-readable result file per run with:

- benchmark name
- implementation name
- total wall time
- external wait time
- orchestration overhead
- audit mode
- replay mode
- retry count
- allocation counters if available
- trace size in bytes
- success / failure

Derived metric:

```text
orchestration_overhead = total_wall_time - external_wait_time
```

External wait time must be explicit and deterministic:

- mocked model calls use fixed synthetic latency
- mocked tool calls use fixed synthetic latency
- retry backoff sleep is reported separately and excluded from orchestration-overhead claims when appropriate

## Workload Families

The suite has four required workload families.

### 1. Tool Loop

Shape:

```text
prompt -> tool -> prompt -> tool -> final structured result
```

Purpose:

- measure repeated orchestration across AI and tool boundaries
- stress prompt/tool scheduling without hiding behind real network latency

Required behavior:

- two model boundaries
- two tool boundaries
- deterministic final JSON result

Mocked external timing:

- model call latency: fixed
- tool call latency: fixed

Reported metrics:

- total wall time
- orchestration overhead
- audit-on vs audit-off
- trace size

### 2. Retry Workflow

Shape:

```text
prompt -> flaky tool -> retry -> retry -> success -> final result
```

Purpose:

- measure retry orchestration cost
- measure bookkeeping cost around deterministic retry policies

Required behavior:

- the tool fails twice
- the third attempt succeeds
- retry policy is fixed and identical across implementations

Mocked external timing:

- failure responses are deterministic
- backoff schedule is fixed

Reported metrics:

- total wall time
- orchestration overhead excluding sleep
- retry bookkeeping overhead
- trace size

### 3. Approval Workflow

Shape:

```text
prompt -> tool proposal -> approval boundary -> tool -> structured result
```

Purpose:

- measure the cost of human-in-the-loop or approval-style safety boundaries
- stress workflow state capture between proposal and execution

Required behavior:

- one model proposes a tool action
- one approval decision is injected deterministically
- the tool runs only after approval

Reported metrics:

- total wall time
- orchestration overhead
- approval-boundary overhead
- audit-on vs audit-off
- replay artifact size

### 4. Replay Trace

Shape:

```text
record one fixed multi-step agent session -> replay it step-by-step
```

Purpose:

- measure the cost of recording, replaying, and inspecting a deterministic AI session
- expose Corvid's replay moat directly

Required behavior:

- the recorded session must include at least one model step and one tool step
- replay must step through the same sequence every run

Reported metrics:

- record cost
- replay cost
- per-step replay latency
- trace size
- determinism check result

## Canonical Fixtures

All implementations should consume the same canonical fixture descriptions.

Recommended repo shape:

```text
benchmarks/
  cases/
    README.md
    schema.json
    tool_loop.json
    retry_workflow.json
    approval_workflow.json
    replay_trace.json
  python/
  typescript/
  corvid/
```

Each fixture file should specify:

- initial user input
- mocked model outputs in order
- mocked tool outputs in order
- fixed synthetic latencies
- expected final structured result
- expected replay event sequence

No implementation may hardcode a different semantic workflow under the same benchmark name.

The canonical files now live under:

- `benchmarks/cases/README.md`
- `benchmarks/cases/schema.json`
- `benchmarks/cases/tool_loop.json`
- `benchmarks/cases/retry_workflow.json`
- `benchmarks/cases/approval_workflow.json`
- `benchmarks/cases/replay_trace.json`

## Audit Modes

Every workload should run in at least two modes:

- `audit_off`
- `audit_on`

For Corvid, `audit_on` means the real ownership verifier and replay/tracing settings intended for production debugging. For competitors, use the closest comparable tracing / audit / instrumentation mode they actually support.

If a competitor lacks a real equivalent:

- record that fact
- keep the implementation in the suite if the workflow still matches
- do not pretend the features are equivalent

## Reporting Format

The publishable table should include:

| Workload | Implementation | Total Time | External Wait | Orchestration Overhead | Audit Mode | Trace Size | Notes |
|---|---|---:|---:|---:|---|---:|---|

And a second summary table for claims:

| Claim | Supporting workloads | Metric |
|---|---|---|
| Corvid lowers AI workflow orchestration overhead | tool loop, retry workflow, approval workflow | orchestration overhead |
| Corvid keeps audit cost low | all workloads in audit-on vs audit-off mode | audit ratio |
| Corvid's replay story is built into execution, not bolted on | replay trace | record + replay cost, determinism check |

## Fairness and Editorial Rules

Do:

- publish the exact commands used
- publish the fixture inputs
- publish both total time and orchestration overhead
- publish limitations explicitly
- rerun on the same machine
- use median, not best run

Do not:

- compare Corvid with tracing/audit on against a competitor with all instrumentation off and call it fair
- benchmark real model latency and claim the runtime won
- hide unsupported replay / audit features in competing stacks
- cherry-pick a single friendly workflow

## Corvid Win Conditions

This suite is successful for Corvid if it can honestly support claims like:

- `Corvid reduces orchestration overhead on replay-audited tool workflows compared with library-built Python and TypeScript stacks.`
- `Corvid keeps runtime audit cost low while preserving deterministic replay.`
- `Corvid's AI-native runtime pays less framework tax than orchestration stacks assembled from libraries.`

The suite does not need to prove that Corvid wins on every generic language benchmark. It needs to prove that Corvid wins in the category it is explicitly designed to own.

## Implementation Plan

Recommended execution order:

1. Finish the memory-foundation close slices that directly affect ownership/audit cost:
   - `17b-2`
   - `17e`
   - `17b-6`
   - `17b-7`
2. Finish the next native-backend wave needed for realistic compiled workflows:
   - `18d`
   - `18e`
3. Add canonical workload fixtures
4. Implement the Corvid runner
5. Implement Python and TypeScript runners
6. Run all implementations on the same hardware
7. Publish only after results are reproducible and the comparison table is complete

## Open Questions

These should be resolved before coding the benchmark harness:

- final Python stack choice
- final TypeScript stack choice
- exact synthetic latencies for model/tool calls
- exact replay artifact schema to compare
- whether a Rust orchestration baseline is worth adding in the first publishable version
