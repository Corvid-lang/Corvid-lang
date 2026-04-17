# Residual orchestration cost breakdown

- Generated: `2026-04-17T11:56:33.778881Z`
- Corvid-only residual breakdown using the instrumented persistent runner

## Control disclosure

Absolute control medians are more informative than coefficient of variation here because the control mean is near zero.

| Session | Control median ms | IQR |
|---|---:|---:|
| `profile` | `0.000244` | `[0.000000, 0.000244]` |
| `trace-on` | `0.000610` | `[0.000488, 0.000732]` |
| `control` | `0.000244` | `[0.000244, 0.000488]` |

## Attribution rule

- `prompt_render`: runtime string helper time used by prompt assembly
- `json_bridge`: prompt bridge overhead after subtracting measured wait and mock dispatch
- `mock_llm_dispatch`: mock lookup and reply construction, excluding sleep
- `trial_init`: per-trial reset/setup inside the persistent native entry loop
- `trace_overhead`: direct trace emit counter inside the runtime
- `rc_release_time`: time spent inside `corvid_release`
- `unattributed`: `orchestration_ms - sum(profiled components)` at the per-trial record level

## Scenario breakdown

### `tool_loop`

- Corvid median orchestration: `0.205238 ms`
- Profiled total median: `0.064601 ms`

| Component | Median ms | % of orchestration |
|---|---:|---:|
| `prompt_render` | `0.009100` | `4.4%` |
| `json_bridge` | `0.040150` | `19.6%` |
| `mock_llm_dispatch` | `0.007400` | `3.6%` |
| `trial_init` | `0.000000` | `0.0%` |
| `trace_overhead` | `0.000000` | `0.0%` |
| `rc_release_time` | `0.006950` | `3.4%` |
| `unattributed` | `0.136826` | `66.7%` |

- Trace-on delta vs trace-off: `+0.002469 ms` (`+1.20%`)
- Profile-session delta vs same-tree control: `-0.117518 ms` (`-36.41%`)

### `retry_workflow`

- Corvid median orchestration: `0.104940 ms`
- Profiled total median: `0.033850 ms`

| Component | Median ms | % of orchestration |
|---|---:|---:|
| `prompt_render` | `0.000000` | `0.0%` |
| `json_bridge` | `0.023100` | `22.0%` |
| `mock_llm_dispatch` | `0.003550` | `3.4%` |
| `trial_init` | `0.000000` | `0.0%` |
| `trace_overhead` | `0.000000` | `0.0%` |
| `rc_release_time` | `0.006500` | `6.2%` |
| `unattributed` | `0.067317` | `64.1%` |

- Trace-on delta vs trace-off: `+0.005512 ms` (`+5.25%`)
- Profile-session delta vs same-tree control: `-0.030232 ms` (`-22.37%`)

### `approval_workflow`

- Corvid median orchestration: `0.060575 ms`
- Profiled total median: `0.029300 ms`

| Component | Median ms | % of orchestration |
|---|---:|---:|
| `prompt_render` | `0.000000` | `0.0%` |
| `json_bridge` | `0.022450` | `37.1%` |
| `mock_llm_dispatch` | `0.003800` | `6.3%` |
| `trial_init` | `0.000000` | `0.0%` |
| `trace_overhead` | `0.000000` | `0.0%` |
| `rc_release_time` | `0.002350` | `3.9%` |
| `unattributed` | `0.032138` | `53.1%` |

- Trace-on delta vs trace-off: `+0.010895 ms` (`+17.98%`)
- Profile-session delta vs same-tree control: `-0.030073 ms` (`-33.18%`)

### `replay_trace`

- Corvid median orchestration: `0.175868 ms`
- Profiled total median: `0.066951 ms`

| Component | Median ms | % of orchestration |
|---|---:|---:|
| `prompt_render` | `0.005100` | `2.9%` |
| `json_bridge` | `0.042750` | `24.3%` |
| `mock_llm_dispatch` | `0.007000` | `4.0%` |
| `trial_init` | `0.000000` | `0.0%` |
| `trace_overhead` | `0.000000` | `0.0%` |
| `rc_release_time` | `0.009750` | `5.5%` |
| `unattributed` | `0.107779` | `61.3%` |

- Trace-on delta vs trace-off: `+0.005747 ms` (`+3.27%`)
- Profile-session delta vs same-tree control: `-0.101272 ms` (`-36.54%`)

## Instrumentation note

The profile-vs-control A/B did not produce a stable overhead estimate. Session-to-session noise on the host was larger than the expected timer tax, so the results are reported as inconclusive rather than proving a <5% measurement overhead.
