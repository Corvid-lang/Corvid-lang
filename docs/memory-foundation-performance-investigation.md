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
- Fine-grained attribution of the residual `13-30 ms` non-wait bucket into prompt rendering vs runtime init vs JSON bridge cost
- Cross-host replication of the same findings on a clean quiet calibration machine

Those are all valid next tools, but none were required to identify the top two contributors in this slice.

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

What changed:

- the Corvid / Python gap narrowed from roughly `25x-36x` to roughly `3x-4x`
- the Corvid / TypeScript gap widened from roughly `1.7x-2.6x` to roughly `8x-10x`

After the direct-counter correction:

- the Corvid / Python gap narrowed further to roughly `1.2x-1.8x`
- the Corvid / TypeScript gap narrowed to roughly `2.4x-4.9x`

Those two moves come from the same correction. The historical session was overstating Corvid's gap to Python by charging startup and wait-accounting artifacts to orchestration cost, and it was understating Corvid's gap to TypeScript by charging Node's sleep overshoot to orchestration cost.

The low-overhead session still does **not** support a claim that Corvid is faster than either stack. It does support a more accurate statement: once the measurable harness artifacts are removed, Corvid's remaining orchestration gap is in the low-single-digit multiples rather than the double-digit multiples that originally published.

For the current published interpretation, see [memory-foundation-results.md](memory-foundation-results.md).
