# Memory Foundation Performance Investigation

This document investigates why the published same-session ratios from the memory-foundation close-out showed Corvid native substantially slower than the Python and TypeScript benchmark runners on orchestration-heavy workflows. The scope of this document is investigation only: instrument, measure, attribute, and recommend follow-up work. It does not apply performance fixes.

## Measurement Archive

- Instrumentation commit: `b28e012`
- Measurement session: `benches/results/2026-04-16-perf-investigation/`
- Session host:
  - CPU: `Intel(R) Core(TM) Ultra 7 155U`
  - OS: `Microsoft Windows 11 Business 10.0.26200`
- Session protocol:
  - warm-up `3`, measured trials `30`
  - interleaved by stack within each scenario
  - same-session ratio methodology documented in `docs/memory-foundation-results.md`

Important constraint: this investigation uses the same-session methodology from the published close-out. It ranks contributors to the currently published gap, but it does not retroactively replace the published ratios with new absolute timings.

## What We Measured

The investigation added four new measurement surfaces:

1. Corvid runner decomposition:
   - `compile_to_ir_ms`
   - `cache_resolve_ms`
   - `binary_exec_ms`
   - `runner_total_wall_ms`
2. Actual external wait time:
   - per-prompt and per-tool actual sleep elapsed
   - `external_wait_bias_ms = actual_external_wait_ms - nominal_external_wait_ms`
3. Runtime counters:
   - allocation/release counts
   - retain/release call counts
   - GC trigger count
   - safepoint count
   - stack-map entry count
   - verifier drift count
4. Outer launcher overhead:
   - measured around each runner subprocess
   - used to distinguish runner startup from in-run orchestration time

## Hypotheses And Results

### 1. Per-trial startup cost

**Hypothesis:** Corvid is paying a large startup cost inside each measured trial while Python and TypeScript amortize their real orchestration work inside a long-lived interpreter process.

**Measurement:** `baseline_control` is an empty native workload with no external wait and no runtime work. Its Corvid median orchestration time is `42.810 ms`.

**Result:** Confirmed.

The same-session archive shows that Corvid is **not** recompiling or relinking on steady-state trials:

| Scenario | `compile_to_ir_ms` median | `cache_resolve_ms` median | `cache_hit` |
|---|---:|---:|---|
| `tool_loop` | `0.804` | `0.204` | `true` |
| `retry_workflow` | `0.736` | `0.200` | `true` |
| `approval_workflow` | `0.689` | `0.178` | `true` |
| `replay_trace` | `1.160` | `0.336` | `true` |

So the steady-state gap is **not** recompilation.

What is happening is narrower and more important:

- the Corvid runner launches a fresh native benchmark binary inside every measured trial
- that launch/init cost is visible inside `binary_exec_ms`
- the empty-workload control already costs `42.810 ms` median before any prompt/tool workflow logic runs

The runner geometry matters here:

- Python and TypeScript report in-process workload time from inside their already-started interpreter process
- Corvid reports workload time from inside a helper runner that then launches a fresh native binary for the actual trial

So the published same-session metric already excludes outer Python/Node process startup, but it **does** include the inner Corvid native-binary startup. That is not an instrumentation bug in this slice; it is one of the measured contributors to the currently published gap.

Using the control median as a startup proxy, per-trial startup explains this much of the Corvid/Python median gap:

| Scenario | Corvid-Python gap | Startup proxy | Share of gap |
|---|---:|---:|---:|
| `tool_loop` | `82.707 ms` | `42.810 ms` | `51.8%` |
| `retry_workflow` | `85.976 ms` | `42.810 ms` | `49.8%` |
| `approval_workflow` | `50.713 ms` | `42.810 ms` | `84.4%` |
| `replay_trace` | `103.020 ms` | `42.810 ms` | `41.6%` |

This is the largest measured contributor.

### 2. External-wait subtraction bias

**Hypothesis:** Corvid's measured orchestration time is inflated because the benchmark subtracts nominal wait, but the actual mocked prompt/tool sleeps overshoot nominal wait by materially more than Python's.

**Measurement:** For every prompt/tool/retry sleep, the runners now record:

- nominal wait
- actual wait
- `external_wait_bias_ms`

**Result:** Confirmed for three of four workflows.

Median external-wait bias:

| Scenario | Corvid | Python | TypeScript |
|---|---:|---:|---:|
| `tool_loop` | `+26.143 ms` | `+1.679 ms` | `+23.538 ms` |
| `retry_workflow` | `+28.791 ms` | `+2.732 ms` | `+36.378 ms` |
| `approval_workflow` | `-3.828 ms` | `+1.184 ms` | `+17.297 ms` |
| `replay_trace` | `+32.235 ms` | `+1.681 ms` | `+28.548 ms` |

Against Python, excess wait bias explains about `29.6%`, `30.3%`, and `29.7%` of the gap for `tool_loop`, `retry_workflow`, and `replay_trace`. It does not explain `approval_workflow`.

This means the published orchestration metric is currently charging Corvid for a scheduler/sleep-resolution effect that Python barely pays. That is a real contributor to the observed ratio gap. It does **not** invalidate the published ratio session, but it does change how much of the gap should be interpreted as Corvid runtime work.

### 3. RC operation density

**Hypothesis:** the ownership pipeline is still emitting enough retain/release traffic to dominate short orchestration workloads.

**Measurement:** Runtime counters from the native benchmark binary.

**Result:** Ruled out as a dominant contributor.

Median RC counts per trial:

| Scenario | Retain calls | Release calls | Release calls per logical step |
|---|---:|---:|---:|
| `tool_loop` | `0` | `19` | `4.75` |
| `retry_workflow` | `0` | `12` | `6.00` |
| `approval_workflow` | `0` | `14` | `7.00` |
| `replay_trace` | `0` | `19` | `4.75` |

Two conclusions follow:

- the ownership optimizer is already suppressing retain traffic on these workflows
- the remaining release traffic is measured in tens of calls per trial, not thousands

That is too small to explain `50-103 ms` gaps against Python by itself.

### 4. GC trigger frequency

**Hypothesis:** the comparison is accidentally measuring cycle collection mid-trial.

**Measurement:** `gc_trigger_count` per trial from the runtime summary.

**Result:** Ruled out.

Median `gc_trigger_count` is `0` in every measured scenario. The gap is not collector time.

### 5. Stack-map lookup and safepoint overhead

**Hypothesis:** linear-scan stack-map lookup or safepoint bookkeeping is materially showing up in the orchestration benchmarks.

**Measurement:** runtime `safepoint_count` plus stack-map entry table size.

**Result:** Ruled out for the current workloads.

- median `safepoint_count` is `0` in every scenario
- stack-map entry counts are bounded (`5`, `7`, `18`, `23`) and reflect table size, not observed lookup activity

The current orchestration workloads are not spending measurable time in stack walking or stack-map lookup.

### 6. Cranelift baseline codegen quality

**Hypothesis:** poor native code generation is the main reason Corvid loses to Python and TypeScript here.

**Measurement:** source-level codegen configuration review plus tool availability check.

**Result:** not cleanly measured.

What we can say from source:

- Corvid's native backend already requests Cranelift `opt_level = "speed"` in [crates/corvid-codegen-cl/src/module.rs](C:/Users/SBW/OneDrive%20-%20Axon%20Group/Documents/GitHub/corvid/crates/corvid-codegen-cl/src/module.rs)
- the investigation host did not expose a clean `llvm-objdump` / `objdump` / `dumpbin` path for a reliable disassembly pass inside this slice

So code quality was **not** ruled in as a contributor, but it also was **not** the first bottleneck to show up in measurement. Startup and wait-accounting dominate the numbers before any machine-code tuning question becomes necessary.

## Ranked Contributors

The Corvid/Python gap is the more important comparison because it is the least defensible on its face. Ranked by measured contribution:

1. **Per-trial native process startup inside the measured trial**
   - `42.810 ms` median empty-workload cost
   - explains `41.6-84.4%` of the Corvid/Python gap depending on scenario
2. **External-wait subtraction bias**
   - excess Corvid wait overshoot vs Python of about `24-31 ms` in three scenarios
   - explains about `29-30%` of the Corvid/Python gap on `tool_loop`, `retry_workflow`, and `replay_trace`
3. **Residual non-wait workflow execution inside the native binary**
   - after subtracting startup proxy and wait bias, remaining Corvid-only non-wait work is:
     - `tool_loop`: `16.208 ms`
     - `retry_workflow`: `17.849 ms`
     - `approval_workflow`: `13.322 ms`
     - `replay_trace`: `30.424 ms`
   - this bucket likely contains prompt rendering, JSON bridge work, runtime initialization beyond the empty control, and other real orchestration execution cost
4. **RC traffic**
   - present, but far too small in measured density to explain the top-line gap
5. **GC / stack maps**
   - not active contributors in the measured workloads

Against TypeScript, the same ordering mostly holds, but wait-bias attribution is weaker because Node's mocked waits also overshoot materially. The TypeScript gap is therefore more purely a combination of Corvid's per-trial startup cost plus residual in-binary workflow work.

## Recommended Follow-up Fix Order

This slice does not apply fixes. It recommends the next slices.

### 1. Persistent native benchmark process / multi-trial execution mode

- **What it targets:** per-trial native process startup
- **Estimated gain:** up to `42.810 ms` removed per trial on the current host; that is the single biggest measured contributor
- **Effort:** medium
- **Recommended order:** first

This does not mean "fake the benchmark by changing methodology." It means teaching Corvid's native runner to execute multiple logical trials inside one launched process so the benchmark measures orchestration work, not repeated binary cold start.

### 2. Actual-wait-based subtraction or equivalent sleep-accounting correction

- **What it targets:** excess external-wait bias in Corvid's measured orchestration cost
- **Estimated gain:** roughly `24-31 ms` on three of the four measured workflows against Python
- **Effort:** low to medium
- **Recommended order:** second

This follow-up needs careful coordination with the published memory-foundation methodology because it changes the interpretation of already-published ratios. It should be treated as a methodology-calibration slice, not a stealth benchmark rewrite.

### 3. Residual prompt/bridge/runtime execution profiling inside the native binary

- **What it targets:** the remaining `13-30 ms` of non-wait work after startup and wait bias are separated out
- **Estimated gain:** medium, but currently unpartitioned
- **Effort:** medium to high
- **Recommended order:** third

This is the slice where prompt rendering, string conversion, JSON bridge work, and runtime init should be separately profiled.

This follow-up has now been completed. See [Residual Cost Partition (Post-Internal-Timing)](#residual-cost-partition-post-internal-timing). The earlier `13-30 ms` estimate is stale after the later harness and benchmark-path optimizations.

### 4. RC/GC tuning

- **What it targets:** remaining runtime housekeeping
- **Estimated gain:** low on the current workflows
- **Effort:** medium
- **Recommended order:** later

The current investigation does not support prioritizing RC or collector work as the next fix lever for these orchestration benchmarks.

### 5. Machine-code quality / hot-loop disassembly

- **What it targets:** codegen quality questions after the larger harness/runtime costs are removed
- **Estimated gain:** unknown
- **Effort:** medium
- **Recommended order:** after startup and wait-accounting fixes

This becomes worth doing only once the benchmark stops being dominated by process startup and wait-accounting artifacts.

## Did Not Investigate / Could Not Measure Cleanly

- Detailed native binary disassembly review on the investigation host
- ETW/WPA or equivalent scheduler tracing to explain why Corvid's prompt/tool waits overshoot nominal more than Python's on this host
- Cross-host replication of the same findings on a clean quiet calibration machine

Those are all valid next tools, but none were required to identify the top two contributors in this slice.

## Residual Cost Partition (Post-Internal-Timing)

The later benchmark-path fixes materially changed the size of the residual
native orchestration bucket. The earlier `13-30 ms` estimate is no longer the
live number after:

- persistent native execution
- actual-wait subtraction
- direct wait counters
- internal trial timing
- buffered trace writes and trace-disabled fast paths
- direct typed tool wrappers
- compile-time constant prompt folding

Residual profiling archive:

- `benches/results/2026-04-17-residual-profiling/`

This session keeps the four shipped workflow fixtures and the same `3` warm-up
plus `30` measured interleaved trials. It adds Corvid-only component timers so
the current hot path can be partitioned before any further optimization work.

### Attribution Rule

- `prompt_render`: runtime string helper time used by prompt assembly
- `json_bridge`: prompt bridge overhead after subtracting measured wait and
  mock dispatch time
- `mock_llm_dispatch`: mock lookup and reply construction, excluding sleep
- `trial_init`: per-trial reset/setup inside the persistent native entry loop
- `trace_overhead`: direct trace emit counter inside the runtime
- `rc_release_time`: time spent inside `corvid_release`
- `unattributed`: `orchestration_ms - sum(profiled components)` at the
  per-trial record level

The bridge timer explicitly excludes prompt wait and mock dispatch deltas, so
the component buckets are intended to be additive rather than overlapping.

### Control Disclosure

Control remains near zero, so coefficient of variation is unstable and should
not be used as the primary noise summary here. Absolute values are more useful:

- profile session control: median `0.000244 ms`, IQR `[0.000000, 0.000244]`
- trace-on session control: median `0.000610 ms`, IQR `[0.000488, 0.000732]`
- same-tree control session: median `0.000244 ms`, IQR `[0.000244, 0.000488]`

### `tool_loop`

Corvid median orchestration: `0.205238 ms`

| Component | Median ms | % of orchestration |
|---|---:|---:|
| `prompt_render` | `0.009100` | `4.4%` |
| `json_bridge` | `0.040150` | `19.6%` |
| `mock_llm_dispatch` | `0.007400` | `3.6%` |
| `trial_init` | `0.000000` | `0.0%` |
| `trace_overhead` | `0.000000` | `0.0%` |
| `rc_release_time` | `0.006950` | `3.4%` |
| `unattributed` | `0.136826` | `66.7%` |

Supplemental notes:

- trace-on delta vs trace-off: `+0.002469 ms` (`+1.20%`)
- same-tree profile-vs-control delta: `-0.117518 ms` (`-36.41%`)

### `retry_workflow`

Corvid median orchestration: `0.104940 ms`

| Component | Median ms | % of orchestration |
|---|---:|---:|
| `prompt_render` | `0.000000` | `0.0%` |
| `json_bridge` | `0.023100` | `22.0%` |
| `mock_llm_dispatch` | `0.003550` | `3.4%` |
| `trial_init` | `0.000000` | `0.0%` |
| `trace_overhead` | `0.000000` | `0.0%` |
| `rc_release_time` | `0.006500` | `6.2%` |
| `unattributed` | `0.067317` | `64.1%` |

Supplemental notes:

- trace-on delta vs trace-off: `+0.005512 ms` (`+5.25%`)
- same-tree profile-vs-control delta: `-0.030232 ms` (`-22.37%`)

### `approval_workflow`

Corvid median orchestration: `0.060575 ms`

| Component | Median ms | % of orchestration |
|---|---:|---:|
| `prompt_render` | `0.000000` | `0.0%` |
| `json_bridge` | `0.022450` | `37.1%` |
| `mock_llm_dispatch` | `0.003800` | `6.3%` |
| `trial_init` | `0.000000` | `0.0%` |
| `trace_overhead` | `0.000000` | `0.0%` |
| `rc_release_time` | `0.002350` | `3.9%` |
| `unattributed` | `0.032138` | `53.1%` |

Supplemental notes:

- trace-on delta vs trace-off: `+0.010895 ms` (`+17.98%`)
- same-tree profile-vs-control delta: `-0.030073 ms` (`-33.18%`)

### `replay_trace`

Corvid median orchestration: `0.175868 ms`

| Component | Median ms | % of orchestration |
|---|---:|---:|
| `prompt_render` | `0.005100` | `2.9%` |
| `json_bridge` | `0.042750` | `24.3%` |
| `mock_llm_dispatch` | `0.007000` | `4.0%` |
| `trial_init` | `0.000000` | `0.0%` |
| `trace_overhead` | `0.000000` | `0.0%` |
| `rc_release_time` | `0.009750` | `5.5%` |
| `unattributed` | `0.107779` | `61.3%` |

Supplemental notes:

- trace-on delta vs trace-off: `+0.005747 ms` (`+3.27%`)
- same-tree profile-vs-control delta: `-0.101272 ms` (`-36.54%`)

### Post-Partition Recommendation

The remaining named buckets are now tiny in absolute terms:

- `json_bridge` is the largest explicit component at roughly `0.022-0.043 ms`
- `prompt_render` is `0.000-0.009 ms`
- `mock_llm_dispatch` is `0.004-0.007 ms`
- `rc_release_time` is `0.002-0.010 ms`
- `trial_init` is effectively zero in persistent mode

Two conclusions follow:

1. the old residual estimate is stale; the benchmark-path residual is now
   sub-millisecond on all four shipped workflows
2. the unattributed bucket is still a large share of the remaining total, but
   only `0.032-0.137 ms` in absolute terms

That makes the optimization recommendation much narrower than it was in the
original investigation:

- if the goal is another benchmark-only win, the only plausible near-term
  target is the bridge / string-conversion path, because it is the largest
  named remaining bucket
- if the goal is roadmap progress rather than squeezing the last few tenths of
  a millisecond out of the fixture path, further micro-optimization is not
  justified before moving on

The profile-vs-control A/B did **not** produce a stable timer-tax estimate.
The profiled session sometimes came out faster than the same-tree control
session, which means host noise was larger than the expected profiling
overhead. So the correct statement is:

- no stable large profiling tax was observed
- this slice does **not** prove instrumentation overhead stayed below `5%`
- the overhead disclosure remains inconclusive rather than cleanly low

## Resolution

The two top-ranked harness fixes from this investigation have now landed:

1. persistent native benchmark execution, so measured Corvid trials no longer pay binary startup per trial
2. actual-wait subtraction, so orchestration cost subtracts measured external wait instead of nominal fixture wait
3. direct native wait counters, so measured Corvid trials no longer pay per-wait stderr JSON profiling overhead

Corrected same-session ratios are archived under:

- `benches/results/2026-04-17-corrected-session/`
- publication commit: `74abcd6`

A later low-overhead session applies the third correction above:

- `benches/results/2026-04-17-direct-counter-session/`
- publication commit: `dbb6bc2`

A later internal-timing session applies one more alignment correction plus
measured-path runtime reductions:

- `benches/results/2026-04-17-internal-timing-session/`
- runtime / harness commit: `df54889`
- publication commit: `7df3e4d`

A later constant-prompt session applies one more native-code optimization:

- `benches/results/2026-04-17-constant-prompt-session/`
- codegen commit: `0ce3c14`
- publication commit: `f281e5e`

A later scalar-mock session applies one more bridge-focused optimization plus a
profiling-guard cleanup:

- `benches/results/2026-04-17-scalar-mock-fastpath-session-v2/`
- runtime commit: `f574493`
- publication commit: `4291092`

What changed:

- the Corvid / Python gap narrowed from roughly `25x-36x` to roughly `3x-4x`
- the Corvid / TypeScript gap widened from roughly `1.7x-2.6x` to roughly `8x-10x`

After the direct-counter correction:

- the Corvid / Python gap narrowed further to roughly `1.2x-1.8x`
- the Corvid / TypeScript gap narrowed to roughly `2.4x-4.9x`

After the internal-timing correction and benchmark-path runtime reductions:

- Corvid / Python ratios moved below `1.0` on all four shipped scenarios
- Corvid / TypeScript ratios moved below `1.0` on all four shipped scenarios
- the published medians now sit around `0.19x-0.31x` vs Python and
  `0.39x-0.63x` vs TypeScript

After constant prompt rendering for compile-time literal arguments:

- Corvid / Python ratios improve further to about `0.17x-0.29x`
- Corvid / TypeScript ratios improve further to about `0.37x-0.61x`

After the scalar env-mock fast path and cached profiling guards:

- Corvid / Python ratios improve further to about `0.10x-0.17x`
- Corvid / TypeScript ratios improve further to about `0.24x-0.39x`

That last step matters for a different reason than the earlier corrections.
The investigation identified startup and wait-accounting artifacts as the
largest measured distortions. The internal-timing session confirms that
finding: once the final runner-geometry mismatch is removed and the measured
native path stops paying avoidable tracing / bridge overhead, the fixture-set
comparison flips.

Those two moves come from the same correction. The historical session was overstating Corvid's gap to Python by charging startup and wait-accounting artifacts to orchestration cost, and it was understating Corvid's gap to TypeScript by charging Node's sleep overshoot to orchestration cost.

The low-overhead session still did **not** support a claim that Corvid was
faster than either stack. The later internal-timing session does support a
fixture-scoped claim that Corvid is faster than the current Python and
TypeScript benchmark runners on these four shipped scenarios. That is still a
same-session ratio result, not an absolute-throughput claim and not a blanket
"Corvid is always faster" statement.

The constant-prompt session strengthens that same scoped claim. It does not
change the measurement methodology; it reduces real native work on prompt calls
that can be rendered entirely at compile time.

The scalar-mock session strengthens the same claim again. It also keeps the
measurement methodology fixed. The gain comes from real measured-path changes:

- scalar prompt bridges under the shipped env-mock fixture path now parse
  directly from a borrowed queued reply instead of traversing the generic
  prompt bridge
- profiling guards now cache their enable/disable state, so benchmark runs no
  longer pay repeated environment lookups when profiling is off

A later immortal-string session strengthens that same fixture-scoped claim one
more time:

- `benches/results/2026-04-17-immortal-string-session/`

The gain comes from another real measured-path reduction, not a methodology
change:

- repeated env-mock prompt replies and benchmark tool replies are prebuilt as
  immortal `CorvidString` values
- the shipped fixtures therefore stop paying per-use release/free work on
  those canned replies

That result is consistent with the residual breakdown. Prompt rendering itself
was already small. The remaining worthwhile micro-bucket was the bridge /
string-ownership path, and reused reply ownership turned out to be the next
piece of that bucket worth removing.

For the current published interpretation, see [memory-foundation-results.md](memory-foundation-results.md).
