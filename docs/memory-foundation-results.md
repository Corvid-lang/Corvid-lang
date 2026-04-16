# Memory Foundation Results

Status: draft close-out.

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

What remains before final lock:

- publish one same-session ratio set for Corvid vs. Python vs. TypeScript
- fold those ratios into this document
- close the roadmap, dev log, learnings, and release tag in one pass

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
orchestration_ms = wall_ms - external_wait_ms
```

`external_wait_ms` is recorded per trial, not assumed globally constant. That matters because even mocked sleep calls have host-dependent wake-up jitter, and the subtraction must reflect the actual measured boundary wait for that trial.

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

- standard deviation of the control scenario across the 30 interleaved measured trials

The reader can then judge whether the observed ratios rise meaningfully above the host's ambient variability.

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
| `17i` | Close-out + benchmarks + release lock | in progress | `-` | this document |

## Open lock conditions

This document locks only when:

- one same-session interleaved ratio run is published under `benches/results/YYYY-MM-DD-ratio-session/`
- the roadmap, dev log, learnings, and release tag are updated together on top of that published session
