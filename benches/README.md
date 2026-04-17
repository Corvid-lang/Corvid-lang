# Benchmark protocol

This directory holds two kinds of benchmark artifacts:

- canonical workflow fixtures under `../benchmarks/cases/`
- runtime / workflow result archives under `results/`

## Same-session ratio protocol

The memory-foundation close publishes **ratios**, not absolute timings.

Why:

- the available host is noisy enough that absolute milliseconds are not yet
  stable enough to publish as canonical lock numbers
- a same-session, interleaved comparison still supports an honest statement
  about Corvid vs. Python / TypeScript orchestration overhead

## Per-scenario session order

For each scenario:

1. run 3 warm-up trials per stack and discard them
2. run 30 measured trials per stack
3. interleave strictly:

```text
Corvid1, Python1, TypeScript1, Corvid2, Python2, TypeScript2, ...
```

Interleaving is mandatory. Running all Corvid trials first and the other
stacks later would let thermal or scheduler drift leak into the language
comparison.

## Trial record format

Each measured trial must record:

- `scenario`
- `stack`
- `trial_idx`
- `wall_ms`
- `external_wait_ms`
- `orchestration_ms`
- `session_id`
- `timestamp`

with:

```text
orchestration_ms = wall_ms - actual_external_wait_ms
```

Each trial keeps both nominal and measured wait:

- `external_wait_ms` = fixture-declared wait
- `actual_external_wait_ms` = measured wait observed by the runner

Publication uses the measured-wait subtraction so wake-up jitter stays in the external-wait bucket instead of being misattributed to orchestration work.

For Corvid's persistent native runner, `wall_ms` must be measured **inside**
the launched native benchmark process from trial start to trial completion.
Do not measure around the parent runner's stdin/stdout request loop, because
Python and TypeScript already report in-process trial elapsed time and the
extra transport overhead would make the comparison asymmetric.

## Published statistics

Each published session emits:

- median Corvid / Python ratio
- median Corvid / TypeScript ratio
- 95% paired bootstrap CI with 10,000 resamples
- `p50`, `p90`, `p99` of the per-trial ratio distribution
- a disclosed session noise floor from the control scenario

## What is intentionally held back

Until a verified-quiet host is available, do not publish:

- absolute milliseconds
- absolute throughput tables
- real-unit latency histograms

Those belong to a later calibration pass, not the same-session close-out.
