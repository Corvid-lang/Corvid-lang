# 2026-04-16 clean-run attempt

This directory captures a reproducibility pass for the memory benchmark
harness in `crates/corvid-runtime/benches/memory_runtime.rs`.

## Command

```powershell
cargo bench -p corvid-runtime --bench memory_runtime -- --warm-up-time 3 --measurement-time 10
```

## Host

- CPU: Intel Core Ultra 7 155U
- Cores / threads: 12 / 14
- OS: Microsoft Windows 11 Business
- OS build: 10.0.26200 (64-bit)

## Noise gate

The acceptance gate for this rerun was:

- run the same command repeatedly on the same machine
- treat `memory_alloc/primitive_control` as the environment-noise sentinel
- reject any run whose `primitive_control` median deviates by more than 5%
  from the other accepted runs

### Primitive-control medians

| Run | Median |
| --- | ---: |
| run-1 | 823.79 us |
| run-2 | 753.35 us |
| run-3 | 758.91 us |
| run-4 | 989.62 us |
| run-5 | 763.59 us |
| run-6 | 452.96 us |

### Outcome

- `run-1`: rejected, +9.3% vs. run-2/run-3 cluster
- `run-2`: accepted by primitive-control gate
- `run-3`: accepted by primitive-control gate
- `run-4`: rejected, +30% vs. run-2/run-3 cluster
- `run-5`: accepted by primitive-control gate, but rejected for publication
  because several non-control benchmarks diverged sharply from the run-2/run-3
  cluster despite the control staying stable
- `run-6`: rejected, -40% vs. run-2/run-3/run-5 cluster

## Publication status

This session does **not** produce publishable lock numbers.

Reason:

- the machine produced cross-sheet instability even when `primitive_control`
  stayed near the accepted cluster
- `run-5` is the clearest example: the control benchmark stayed within the
  5% gate, but `tight_box_alloc`, `retain_release_pair`, and several collector
  timings diverged too far from runs 2 and 3 to treat the session as quiet

The raw outputs are still useful as harness-validation artifacts:

- the repaired benchmark harness compiles and runs end-to-end
- the raw Criterion outputs are preserved for audit
- the next clean rerun should be performed on a quieter machine or under a
  tighter process/power-management envelope before any numbers are copied into
  `docs/memory-foundation-results.md`

## Best observed stable cluster

Runs 2 and 3 were the closest pair and are the only runs in this session that
look mutually consistent across most of the sheet. They are preserved here for
review, but they are **not** treated as the final published result set because
the session never reached three clean, mutually consistent runs.
