# Memory Foundation Results

Status: closed on `v0.1-memory-foundation`.

The memory-management foundation is shipped on `main`, including:

- typed heap headers and per-type metadata
- native mark-sweep cycle collection
- interpreter-tier Bacon-Rajan cycle collection
- weak references with effect-typed invalidation
- replay-deterministic GC trigger logging
- runtime refcount verification with blame PCs
- the default-on unified ownership pass
- drop specialization
- effect-typed scope reduction
- latency-aware RC at prompt / LLM boundaries

This document is the receipt for the shipped foundation. It is not the end of the optimization story, and it does not pretend the deferred research work already landed.

## Methodology: same-session ratios

Corvid's AI-native positioning is a comparative claim:

> compile-time safety with reasonable performance relative to Python and TypeScript orchestration code

That claim is best measured by **same-session ratios**, not by absolute wall-clock timings from a noisy laptop.

### Why ratios, not absolutes

The benchmark host available during the close produced measurable ambient noise:

- background scheduler jitter
- thermal drift
- intermittent indexing / package-cache interference

Those effects make absolute microseconds untrustworthy enough to hold back from publication. They do **not** invalidate a same-session comparison when all three stacks are run interleaved in one session, because the same drift hits each stack under the same host conditions.

So the publication rule is:

- publish **ratios**
- hold **absolute** timings until a verified-quiet host is available

### Protocol

For each scenario:

1. warm up each stack with 3 discarded trials
2. run 30 measured trials per stack
3. interleave the measured trials strictly:

```text
C1, P1, T1, C2, P2, T2, ... C30, P30, T30
```

where:

- `C` = Corvid native
- `P` = Python
- `T` = TypeScript / Node

Interleaving is mandatory. Running all Corvid trials first and Python / TypeScript later would let session drift masquerade as a language effect.

### Recorded fields

Each measured trial is recorded as one JSONL line with:

- `scenario`
- `stack`
- `trial_idx`
- `wall_ms`
- `external_wait_ms`
- `orchestration_ms`
- `session_id`
- `timestamp`

where:

```text
orchestration_ms = wall_ms - actual_external_wait_ms
```

The trial record keeps both:

- `external_wait_ms` = nominal wait requested by the fixture
- `actual_external_wait_ms` = measured wait observed at runtime

As of commit `e5b371a`, the published orchestration metric subtracts the **measured** wait, not the nominal wait. Prior published same-session ratios used nominal subtraction and remain archived as-is for reproducibility.

That distinction matters because even mocked sleep calls have host-dependent wake-up jitter. Subtracting nominal wait charges overshoot or undershoot to orchestration cost; subtracting actual measured wait isolates the language/runtime work more faithfully.

As of commit `df54889`, Corvid's persistent native runner also measures
`wall_ms` inside the launched native benchmark process from trial start to
trial completion. That aligns Corvid with the Python and TypeScript runners,
which already reported in-process trial elapsed time instead of outer
stdin/stdout transport cost.

### Published statistics

For each scenario, the published result is:

- median Corvid / Python orchestration ratio
- median Corvid / TypeScript orchestration ratio
- 95% bootstrap CI for each ratio, with 10,000 paired resamples by `trial_idx`
- ratio-shape summary: `p50`, `p90`, `p99`
- session noise-floor disclosure from the control scenario

Interpretation rule:

- if the 95% CI overlaps `1.0`, the session does **not** support a claim that Corvid is measurably different for that scenario

### Noise-floor disclosure

Each published session includes a single disclosed noise floor:

- coefficient of variation of the control scenario across the 30 interleaved measured trials

The reader can then judge whether the observed ratios rise meaningfully above the host's ambient variability.

When the control median approaches zero, the coefficient of variation becomes unstable because the denominator is too small. In those sessions, the archive also publishes the control median and IQR in absolute milliseconds and treats those absolute values as the primary control disclosure.

### What is intentionally not published yet

Until a verified-quiet host is available, this close-out does **not** publish:

- absolute milliseconds
- throughput-per-second claims
- absolute latency histograms

Those will land later as a separate calibration pass. The memory-foundation close publishes only what the current host can support honestly.

## Foundation summary

What shipped:

- native and VM cycle collection
- runtime refcount verification
- weak references
- unified ownership optimization
- prompt-boundary RC flattening
- shared cross-language benchmark fixtures
- native Corvid, Python, and TypeScript workflow runners

What this foundation claims:

- Corvid can execute replay-aware, tool-calling, approval-aware workflows with integrated ownership tracking
- Corvid can compare itself honestly against Python and TypeScript on orchestration overhead rather than network latency
- Corvid now has a reproducible measurement surface for its memory and ownership story

What this foundation does **not** claim:

- multi-threaded RC
- region allocation
- reuse-shape specialization
- effect-row-directed RC proof at the granularity originally proposed for the optimization wave

## Comparative runner surface

The comparative runner set is now in-repo:

| Implementation | Directory | Status |
|---|---|---|
| Corvid native | `benches/corvid/` | shipped |
| Python stdlib | `benches/python/` | shipped |
| TypeScript / Node | `benches/typescript/` | shipped |

All three consume the canonical fixtures under `benchmarks/cases/` and emit JSONL trial records using the same subtraction rule.

## Same-session ratio sessions

### Current internal-timing session

Source archive:

- `benches/results/2026-04-17-internal-timing-session/`
- runtime / harness commit: `df54889`
- publication commit: `7df3e4d`

Methodology and measured-path changes relative to the earlier low-overhead session:

- Corvid `wall_ms` is now measured inside the native benchmark process from
  trial start to trial completion instead of around the parent runner's
  stdin/stdout request loop.
- measured Corvid runs keep the earlier direct wait counters and also remove
  remaining benchmark-path runtime overhead:
  - buffered trace writes
  - trace-disabled fast path that skips event construction entirely
  - direct typed tool wrappers for the fixture tools
  - mock prompt fast path that avoids unused bridge work

This session still publishes only same-session ratios. Absolute milliseconds
remain held until a verified-quiet host is available.

Session disclosure:

- control values are close to zero on all three stacks, so CV is unstable as a
  primary noise summary
- absolute control disclosure:
  - `corvid`: median `0.000244 ms`, IQR `[0.000244, 0.000488]`, CV `66.32%`
  - `python`: median `0.001100 ms`, IQR `[0.000900, 0.001400]`, CV `29.70%`
  - `typescript`: median `0.001100 ms`, IQR `[0.000700, 0.001400]`, CV `75.39%`

#### Low-overhead vs internal-timing medians

| Scenario | Corvid / Python low-overhead | Corvid / Python internal-timing | Corvid / TypeScript low-overhead | Corvid / TypeScript internal-timing |
|---|---:|---:|---:|---:|
| `tool_loop` | `1.818` | `0.284` | `3.608` | `0.611` |
| `retry_workflow` | `1.188` | `0.186` | `2.378` | `0.392` |
| `approval_workflow` | `1.787` | `0.286` | `4.938` | `0.626` |
| `replay_trace` | `1.695` | `0.312` | `2.956` | `0.608` |

#### Corvid vs Python

| Scenario | Median ratio | 95% CI |
|---|---:|---:|
| `tool_loop` | `0.284` | `[0.255, 0.305]` |
| `retry_workflow` | `0.186` | `[0.174, 0.197]` |
| `approval_workflow` | `0.286` | `[0.261, 0.305]` |
| `replay_trace` | `0.312` | `[0.285, 0.333]` |

#### Corvid vs TypeScript

| Scenario | Median ratio | 95% CI |
|---|---:|---:|
| `tool_loop` | `0.611` | `[0.529, 0.657]` |
| `retry_workflow` | `0.392` | `[0.351, 0.421]` |
| `approval_workflow` | `0.626` | `[0.536, 0.740]` |
| `replay_trace` | `0.608` | `[0.547, 0.701]` |

Interpretation:

- every internal-timing ratio is below `1.0`
- every internal-timing 95% CI stays below `1.0`
- so this session supports a stronger claim than the earlier archives:
  Corvid is faster than the current Python and TypeScript benchmark runners on
  these four shipped workflow fixtures

What this session supports:

- Corvid now leads both comparison stacks on the fixture workloads used in the
  published runner suite
- the earlier Python gap was dominated by harness artifacts and benchmark-path
  overhead, not by an inherent orchestration ceiling in native Corvid
- once runner geometry is aligned and measured-path overhead is removed, the
  current Corvid native path is competitive enough to win the shipped fixture
  set

What this session does **not** support:

- a universal claim that Corvid is faster than Python or Node orchestration in
  every workload shape
- any absolute-millisecond statement; this remains a ratio-only result on a
  noisy host

The internal-timing archive includes the full ratio-shape tables (`p50` /
`p90` / `p99`) in `ratios.md`. This document keeps only the headline medians
and confidence intervals.

### Earlier low-overhead harness session

Source archive:

- `benches/results/2026-04-17-direct-counter-session/`
- publication commit: `dbb6bc2`

Methodology corrections relative to the prior corrected harness session:

- prompt and tool wait accounting now comes from direct per-trial native counters in the benchmark summary
- ordinary measured Corvid runs no longer emit per-wait profiling JSON to stderr

This does not change the orchestration metric. It removes Corvid-only serialization and parsing overhead from the measured path.

Session disclosure:

- control values are close to zero on all three stacks, so CV is unstable as a primary noise summary
- absolute control disclosure:
  - `corvid`: median `0.11265 ms`, IQR `[0.08502, 0.12668]`, CV `46.41%`
  - `python`: median `0.00130 ms`, IQR `[0.00110, 0.00150]`, CV `25.60%`
  - `typescript`: median `0.00125 ms`, IQR `[0.00103, 0.00213]`, CV `65.58%`

#### Corrected vs low-overhead medians

| Scenario | Corvid / Python corrected | Corvid / Python low-overhead | Corvid / TypeScript corrected | Corvid / TypeScript low-overhead |
|---|---:|---:|---:|---:|
| `tool_loop` | `3.528` | `1.818` | `8.338` | `3.608` |
| `retry_workflow` | `2.978` | `1.188` | `7.657` | `2.378` |
| `approval_workflow` | `3.486` | `1.787` | `10.207` | `4.938` |
| `replay_trace` | `3.780` | `1.695` | `8.531` | `2.956` |

#### Corvid vs Python

| Scenario | Median ratio | 95% CI |
|---|---:|---:|
| `tool_loop` | `1.818` | `[1.628, 1.938]` |
| `retry_workflow` | `1.188` | `[1.127, 1.305]` |
| `approval_workflow` | `1.787` | `[1.628, 1.940]` |
| `replay_trace` | `1.695` | `[1.485, 1.817]` |

#### Corvid vs TypeScript

| Scenario | Median ratio | 95% CI |
|---|---:|---:|
| `tool_loop` | `3.608` | `[3.015, 4.173]` |
| `retry_workflow` | `2.378` | `[2.145, 2.732]` |
| `approval_workflow` | `4.938` | `[3.420, 5.589]` |
| `replay_trace` | `2.956` | `[2.513, 3.609]` |

Historical interpretation:

- every low-overhead ratio is still greater than `1.0`
- every low-overhead 95% CI stays above `1.0`
- so this session did **not** support any claim that Corvid was faster than
  either Python or TypeScript on these workflow runners

What this session does support:

- Corvid is now within roughly `1.2x-1.8x` of Python on these orchestration workloads
- Corvid is now within roughly `2.4x-4.9x` of TypeScript / Node on these orchestration workloads
- the remaining gap is now small enough to treat as ordinary optimization work rather than a benchmark-validity crisis
- this is the first session that is plausibly useful for both developer evaluation and early marketing, provided the claim stays at "competitive overhead with stronger safety" rather than "faster than the glue stacks"

The low-overhead archive remains useful as an intermediate calibration point.
It is no longer the preferred interpretation because the internal-timing
session removes the last large runner-geometry mismatch.

### Corrected harness session

Source archive:

- `benches/results/2026-04-17-corrected-session/`
- publication commit: `74abcd6`

Methodology corrections relative to the historical close-out session:

- Corvid now executes measured trials inside a persistent native process instead of relaunching the binary for every trial.
- All three stacks now subtract **actual measured external wait**, not nominal fixture wait.

Session disclosure:

- control values are close to zero on all three stacks, so CV is unstable as a primary noise summary
- absolute control disclosure:
  - `corvid`: median `0.1017 ms`, IQR `[0.0919, 0.1149]`, CV `216.89%`
  - `python`: median `0.00135 ms`, IQR `[0.00095, 0.00177]`, CV `38.43%`
  - `typescript`: median `0.00080 ms`, IQR `[0.00060, 0.00147]`, CV `79.29%`

#### Before / after medians

| Scenario | Corvid / Python historical | Corvid / Python corrected | Corvid / TypeScript historical | Corvid / TypeScript corrected |
|---|---:|---:|---:|---:|
| `tool_loop` | `36.126` | `3.528` | `2.627` | `8.338` |
| `retry_workflow` | `24.985` | `2.978` | `1.669` | `7.657` |
| `approval_workflow` | `25.635` | `3.486` | `2.488` | `10.207` |
| `replay_trace` | `35.326` | `3.780` | `2.636` | `8.531` |

The same methodology correction moved the story in opposite directions:

- Corvid vs Python improved sharply because the historical session was charging repeated native startup and wait-accounting bias to orchestration cost.
- Corvid vs TypeScript worsened sharply because the historical session was also charging Node's real sleep overshoot to orchestration cost.

#### Corvid vs Python

| Scenario | Median ratio | 95% CI |
|---|---:|---:|
| `tool_loop` | `3.528` | `[3.263, 4.046]` |
| `retry_workflow` | `2.978` | `[2.872, 3.165]` |
| `approval_workflow` | `3.486` | `[3.146, 3.795]` |
| `replay_trace` | `3.780` | `[3.571, 4.124]` |

#### Corvid vs TypeScript

| Scenario | Median ratio | 95% CI |
|---|---:|---:|
| `tool_loop` | `8.338` | `[7.460, 9.828]` |
| `retry_workflow` | `7.657` | `[7.241, 8.033]` |
| `approval_workflow` | `10.207` | `[9.162, 10.851]` |
| `replay_trace` | `8.531` | `[7.618, 9.571]` |

Interpretation:

- every corrected ratio is still greater than `1.0`
- every corrected 95% CI stays above `1.0`
- so the corrected session still does **not** support any claim that Corvid is faster than either Python or TypeScript on these workflow runners

What the corrected session does support:

- the published historical Corvid / Python gap was materially overstated by harness artifacts
- after removing those artifacts, Corvid's orchestration overhead is within roughly `3x-4x` of Python on these scenarios
- after the same correction, Corvid is still materially slower than TypeScript / Node on these scenarios, at roughly `8x-10x`
- this is a defensible `v0.1` position with clear optimization headroom, not a performance-win story

The corrected archive includes the full ratio-shape tables (`p50` / `p90` / `p99`) in `ratios.md`. This document keeps only the headline medians and confidence intervals.

### Historical close-out session

Source archive:

- `benches/results/2026-04-16-ratio-session/`
- publication commit: `4090366`

Historical session disclosure:

- noise floor: `41.40%` control CV on the worst stack
- per-stack control CV:
  - `corvid`: `20.90%`
  - `python`: `41.40%`
  - `typescript`: `28.69%`

Historical medians:

| Scenario | Corvid / Python | Corvid / TypeScript |
|---|---:|---:|
| `tool_loop` | `36.126` | `2.627` |
| `retry_workflow` | `24.985` | `1.669` |
| `approval_workflow` | `25.635` | `2.488` |
| `replay_trace` | `35.326` | `2.636` |

This session remains archived unchanged for reproducibility. It is no longer the preferred interpretation because it was collected before the persistent-process correction and before actual-wait subtraction replaced nominal subtraction.

## Optimization wave

The optimization wave supports Corvid's actual category claim:

> replay-deterministic execution with low audit cost

Shipped optimization stages in this close:

- unified ownership pass default-on
- whole-program pair elimination
- drop specialization
- effect-typed scope reduction
- latency-aware RC at prompt boundaries

Important measured finding that shaped the final scope:

- borrowed-local tool boundaries were already close to flat after the ownership pass became default-on
- the remaining AI-boundary RC hotspot lives at prompt / LLM boundaries, not generic tool dispatch

That is why the shipped boundary optimization is prompt-focused rather than trying to claim a generic "all AI boundaries got cheaper" story.

## Deferred work

Deferred research remains explicit:

- reuse analysis
- Morphic-style alias-mode specialization
- escape analysis
- VM collector locality work

And the next native-backend wave remains separate:

- native tagged-union lowering for `Result` / `Option` / `?`
- native retry runtime / codegen support

See [memory-foundation-deferrals.md](memory-foundation-deferrals.md) for the rationale rather than duplicating it here.

## Implementation map

| Slice | Description | Status | Commit | Notes |
|---|---|---|---|---|
| `17a` | Typed heap headers + per-type typeinfo + non-atomic RC | shipped | `1fea6a0` | foundation |
| `17b-0` | Retain/release counters + baseline RC counts | shipped | `7ef4304` | measurement baseline |
| `17b-1a` | `Dup` / `Drop` IR + borrow signature scaffolding | shipped | `82f78b5` | scaffolding |
| `17b-1b.1` | Borrow inference + callee-side ABI elision | shipped | `2bce2a8` | ownership groundwork |
| `17b-1b.2` | String borrow-at-use-site peephole | shipped | `71c7fe4` | ownership groundwork |
| `17b-1b.3` | `FieldAccess` / `Index` borrow-at-use-site peephole | shipped | `de3acb5` | ownership groundwork |
| `17b-1b.4` | `for` iterator borrow-at-use-site peephole | shipped | `a725449` | ownership groundwork |
| `17b-1b.5` | Call-arg borrow-at-use-site peephole | shipped | `b0a911e` | ownership groundwork |
| `17b-1b.6a` | CFG + liveness + ownership dataflow analysis | shipped | `760b07e` | unified-pass groundwork |
| `17b-1b.6b` | IR `Dup` / `Drop` insertion from analysis | shipped | `1d1af44` | unified-pass groundwork |
| `17b-1b.6c` | Hook ownership pass into codegen pipeline | shipped | `f3762cd` | opt-in stage |
| `17b-1b.6d-1` | Guard scattered emit sites + runtime flag | shipped | `8e2e98e` | transition stage |
| `17b-1b.6d-2a` | Entry-main + drop-before-return + BinOp consume | shipped | `520e30b` | transition stage |
| `17b-1b.6d-2` | Unified ownership pass default-on | shipped | `0cc7895` | default path |
| `17b-1c` | Whole-program pair elimination | shipped | `046806d` | first ARC-style pair pass |
| `17b-2` | Drop specialization | shipped | `8c55c3f` | close-of-foundation optimization |
| `17b-3` | Reuse analysis | deferred | `-` | research-tier |
| `17b-4` | Morphic-style specialization | deferred | `-` | research-tier |
| `17b-5` | Escape analysis | deferred | `-` | research-tier |
| `17b-6` | Effect-row-directed RC | deferred to next wave | `-` | effect system too simple for a sound close slice |
| `17b-7` | Latency-aware RC across prompt / LLM boundaries | shipped | `6bedbfb` | prompt hotspot |
| `17c` | Safepoints + stack-map emission | shipped | `e55efea` | native GC roots |
| `17d` | Native mark-sweep cycle collector | shipped | `ca428bf` | native cycles |
| `17e` | Effect-typed scope reduction | shipped | `f5a3bce` | conservative same-block relocation |
| `17f / 17f++` | Deterministic GC triggers + refcount verifier | shipped | `a3b841d` | verifier |
| `17g` | `Weak<T>` with effect-typed invalidation | shipped | `ba01e78` | VM and native safety story |
| `17h.1` | VM-owned heap handles | shipped | `318c892` | VM memory refactor |
| `17h.2` | VM Bacon-Rajan cycle collector | shipped | `91d95ac` | interpreter parity |
| `17i` | Close-out + benchmarks + release lock | shipped | `v0.1-memory-foundation` | this document + release tag |
