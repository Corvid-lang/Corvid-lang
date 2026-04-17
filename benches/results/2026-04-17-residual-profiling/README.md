## Residual Orchestration Profiling Session

- Session ID: `2026-04-17-residual-profiling`
- Host:
  - `Dell Latitude 5550`
  - `Intel(R) Core(TM) Ultra 7 155U` (`12` cores / `14` logical processors)
  - `16,597,598,208` bytes RAM
  - `Microsoft Windows 11 Business` `10.0.26200` (build `26200`)

## Protocol

- Warm-up: `3` discarded trials per stack per scenario
- Measured: `30` trials per stack per scenario
- Interleaving: `Corvid -> Python -> TypeScript` repeated by `trial_idx`
- Corvid process mode: `persistent`
- Profiling mode: `CORVID_BENCH_PROFILE=1`
- Trace mode for this archive: `CORVID_BENCH_TRACE_DISABLE=1`
- Published metric inside the raw session: `orchestration_ms = wall_ms - actual_external_wait_ms`

This archive is the post-internal-timing residual-cost follow-up. It does not
change the shipped benchmark scenarios or the trial count. It adds fine-grained
Corvid-only component timers so the remaining native hot-path cost can be
partitioned before deciding whether more micro-optimization is worth it.

## Files

- `raw.jsonl`: canonical interleaved session records
- `components.json`: machine-readable residual component summary
- `components.md`: reviewer-facing breakdown tables

Supplemental sessions used for interpretation:

- `../2026-04-17-residual-profiling-trace-on/raw.jsonl`
  - same protocol with tracing enabled to estimate trace-path delta
- `../2026-04-17-residual-control/raw.jsonl`
  - same-tree control run without profiling to check whether the timers add
    obvious session-scale overhead

## Attribution Rule

- `prompt_render`: runtime string helper time used by prompt assembly
- `json_bridge`: prompt bridge overhead after subtracting measured wait and
  mock dispatch time
- `mock_llm_dispatch`: mock reply lookup and construction, excluding sleep
- `trial_init`: per-trial reset/setup inside the persistent native entry loop
- `trace_overhead`: direct trace emit counter inside the runtime
- `rc_release_time`: time spent inside `corvid_release`
- `unattributed`: `orchestration_ms - sum(profiled components)` at the
  per-trial record level

Components are mutually exclusive by construction. The bridge timer excludes
prompt wait and mock-dispatch deltas, so those costs are not double-counted in
the component table.

## Control Disclosure

The control scenario is now extremely close to zero, so coefficient of
variation is unstable as a noise summary. The absolute control values are more
useful:

- profile session control: median `0.000244 ms`, IQR `[0.000000, 0.000244]`
- trace-on session control: median `0.000610 ms`, IQR `[0.000488, 0.000732]`
- same-tree control session: median `0.000244 ms`, IQR `[0.000244, 0.000488]`

## High-Level Outcome

The residual orchestration bucket on the current fast native benchmark path is
already sub-millisecond across all four shipped workflows:

- `tool_loop`: `0.205238 ms`
- `retry_workflow`: `0.104940 ms`
- `approval_workflow`: `0.060575 ms`
- `replay_trace`: `0.175868 ms`

Largest named remaining component:

- `json_bridge`: about `0.022-0.043 ms` depending on scenario

Other measured components are small in absolute terms:

- `prompt_render`: `0.000-0.009 ms`
- `mock_llm_dispatch`: `0.004-0.007 ms`
- `rc_release_time`: `0.002-0.010 ms`
- `trial_init`: `0.000 ms`

The unattributed remainder is still a large share of the now-tiny orchestration
total, but only `0.032-0.137 ms` in absolute terms.

## Instrumentation Overhead

The profile-vs-control A/B did not produce a stable timer-tax estimate. The
same-tree profiled session sometimes came out faster than the control session,
which means host noise was larger than the expected profiling overhead.

That result should be read as:

- no obvious large profiling tax was observed
- the session does **not** prove that profiling overhead is below `5%`
- overhead is therefore disclosed as inconclusive rather than low with
  confidence

## Commands

```powershell
python benches/analysis/session.py --output-dir benches/results/2026-04-17-residual-profiling --warmup 3 --trials 30 --profile
$env:CORVID_BENCH_TRACE_DISABLE='0'
python benches/analysis/session.py --output-dir benches/results/2026-04-17-residual-profiling-trace-on --warmup 3 --trials 30 --profile
Remove-Item Env:CORVID_BENCH_TRACE_DISABLE
python benches/analysis/session.py --output-dir benches/results/2026-04-17-residual-control --warmup 3 --trials 30
python benches/analysis/residual_breakdown.py benches/results/2026-04-17-residual-profiling/raw.jsonl benches/results/2026-04-17-residual-profiling-trace-on/raw.jsonl benches/results/2026-04-17-residual-control/raw.jsonl benches/results/2026-04-17-residual-profiling/components.json benches/results/2026-04-17-residual-profiling/components.md
```
