# Memory Foundation Results

Status: draft close-out. The memory-management foundation is shipped on `main`, including the default-on unified ownership pass (`0cc7895`). Final lock is intentionally held until the remaining foundation-close slices land:

- `17b-2` drop specialization
- `17e` effect-typed scope reduction
- `17b-6` effect-row-directed RC
- `17b-7` latency-aware RC across tool / LLM boundaries

This is Corvid's memory-management foundation:

- typed heap headers and per-type metadata
- native mark-sweep cycle collection
- interpreter-tier Bacon-Rajan cycle collection
- weak references with effect-typed invalidation
- replay-deterministic GC trigger logging
- runtime refcount verification with blame PCs
- a unified ownership pass that is now the default codegen path

This document is the receipt for that foundation. It is not the end of the optimization story. The ownership optimization wave closes the foundation powerfully. The research-tier items remain explicitly deferred.

## Methodology

Command surface:

```bash
cargo bench -p corvid-runtime --bench memory_runtime -- --sample-size 10 --warm-up-time 1 --measurement-time 3
```

Measurement discipline:

- Criterion median + IQR
- warm-up excluded
- deterministic fixed-size workloads
- collector timing excludes graph construction
- native and VM collectors are measured separately because they are different systems
- cold-cache allocation path uses deterministic cache-thrash preload instead of a friendlier hot-cache-only number

Baseline references:

- historical narrative baseline: the earlier native-runtime close-out numbers from `dev-log.md` Day 29
- code baseline before the memory-foundation work: `ca46e49de7140631c2fb0cb19247f97a9a111227`
- current post-unified-pass baseline: `0cc7895`

## Act I - Foundation

### Current post-`0cc7895` measurements

These are the current measured numbers on the tree where the unified ownership pass is default-on.

#### Allocation-path benchmarks

These are allocator-path numbers, not collector numbers. The goal is to isolate the cost of creating and dropping refcounted values in the hot path.

| Benchmark | Median | Throughput | Derived note |
|---|---:|---:|---|
| `tight_box_alloc` | `236.54 us` | `84.553 M allocs/sec` | about `11.8 ns/alloc` over `20,000` allocs |
| `tight_box_alloc_cold_preload` | `401.71 us` | `49.787 M allocs/sec` | about `20.1 ns/alloc` after deterministic cold-cache preload |
| `string_heavy_concat` | `202.82 us` | `19.732 M elems/sec` | String-heavy intermediate allocation path |
| `list_heavy_strings` | `1.8430 ms` | `9.1677 M elems/sec` | composite path with refcounted elements |
| `primitive_control` | `343.25 us` | `5.8266 G ops/sec` | control workload with no RC traffic |

Claim supported:

- Corvid's hottest fixed-size native allocation path is now materially stronger on the post-unified-pass tree, and the cold-cache validation path remains in the low-20-ns range instead of collapsing the story.

#### Isolated RC-op cost

| Benchmark | Median | Derived cost |
|---|---:|---:|
| `rc_ops_isolated/retain` | `3.6912 ms / 1,000,000` | about `3.69 ns/retain` |
| `rc_ops_isolated/release` | `3.5602 ms / 1,000,000` | about `3.56 ns/release` |
| `rc_ops_isolated/retain_release_pair` | `2.3826 ms / 1,000,000` | about `2.38 ns/pair` |

Claim supported:

- The optimization wave can now be judged honestly. Future ownership-pass wins can be translated into a real per-call cost reduction rather than hand-waving about "fewer RC ops."

#### Collector-path benchmarks

These are collector-path numbers. They should not be conflated with allocator-path wins. In particular, the native sweep path now returns fixed-size blocks to the bounded pool instead of always calling `free()`, so these measurements reflect the current runtime end-to-end collection path, not a "pure mark/sweep with no allocator interaction" thought experiment.

| Native mark-sweep size | Median | Per-node |
|---|---:|---:|
| `10` | `99.283 ns` | `9.93 ns/node` |
| `100` | `805.01 ns` | `8.05 ns/node` |
| `1,000` | `8.5868 us` | `8.59 ns/node` |
| `10,000` | `91.297 us` | `9.13 ns/node` |

Claim supported:

- Native cycle collection has the expected `O(reachable + unreachable)` shape and now sits around the high-single-digit nanoseconds per node on the current runtime path.

#### VM collector throughput

| VM Bacon-Rajan size | Median | Per-node |
|---|---:|---:|
| `10` | `5.8622 us` | `0.586 us/node` |
| `100` | `58.134 us` | `0.581 us/node` |
| `1,000` | `654.52 us` | `0.655 us/node` |
| `10,000` | `7.3613 ms` | `0.736 us/node` |

Claim supported:

- Interpreter-tier cycle collection now works on real cyclic graphs without recursive stack growth and stays under `1 us/node` through the smaller graph sizes in the current run.

Important honesty note:

- VM and native collectors remain independent implementations. Parity is behavioural and asserted by tests, not by a shared allocator.

#### Verifier overhead

| Workload | `off` median | `warn` median | Ratio | Note |
|---|---:|---:|---:|---|
| `tight_box_alloc` | `498.35 us` | `559.79 us` | `1.12x` | alloc-heavy hot path |
| `string_heavy_concat` | `254.35 us` | `254.94 us` | `1.00x` | effectively noise in this run |
| `list_heavy_strings` | `1.9096 ms` | `2.3653 ms` | `1.24x` | wide spread in `warn`, including severe high outliers |

Claim supported:

- Corvid's runtime ownership verifier is clearly CI-usable on the post-unified-pass tree while preserving deterministic trigger logging and blame PCs.

Residual profile:

- the big win was moving verifier scratch state into the tracking node
- the unified ownership pass reduced the audited RC traffic again on top of that
- the remaining interesting cost is on composite/list-heavy paths, where the current `warn` run still shows variance instead of the tighter alloc-heavy profile

### Claim-to-number mapping

| Architectural claim | Supporting measurement |
|---|---|
| Corvid can audit ownership optimizer output at runtime without prohibitive cost | verifier `warn/off` ratios above |
| Corvid's fixed-size allocator path is now genuinely competitive under both hot and cold-cache setup | `tight_box_alloc` + `tight_box_alloc_cold_preload` |
| Corvid's native cycle collector is production-shaped, not just correct | native mark-sweep per-node table |
| Corvid's interpreter replay tier no longer hides a recursion hazard in collection | deep-cycle regression test + VM collector table |
| Ownership optimization work can be evaluated quantitatively | isolated retain / release / pair benchmarks |

## Act II - Optimization Wave

The optimization wave supports Corvid's actual category claim:

> replay-deterministic execution with low audit cost

### Shipped optimization slices so far

#### `17b-1b.6d-2` - unified ownership pass default-on (`0cc7895`)

What it does:

- makes the ownership pass the default codegen path
- removes the old "opt-in experiment" framing
- lowers measured RC traffic enough that both hot allocation and verifier costs move again on the same harness

Measured delta vs the pre-`0cc7895` draft baseline:

| Benchmark | Pre-`0cc7895` | Post-`0cc7895` | Delta |
|---|---:|---:|---:|
| `tight_box_alloc` | `30.6 ns/alloc` | `11.8 ns/alloc` | `2.58x` faster |
| `tight_box_alloc_cold_preload` | `37.9 ns/alloc` | `20.1 ns/alloc` | `1.88x` faster |
| `native_mark_sweep/10000` | `14.6 ns/node` | `9.13 ns/node` | `1.60x` faster |
| verifier `tight_box_alloc` ratio | `1.22x` | `1.12x` | lower audit overhead |
| verifier `string_heavy_concat` ratio | `1.26x` | `1.00x` | effectively eliminated in this run |

Headline unlocked:

- Corvid's verified RC path is now markedly cheaper with the unified ownership pass live by default.

#### `17b-1c` - whole-program pair elimination (`046806d`)

What it does:

- removes same-block `Dup` / `Drop` pairs when one safe internal use sits between them and nothing else touches the local
- explicitly refuses to pair across branches, loops, weak creation, or external call boundaries

Measured delta today:

- the current published `baseline_rc_counts` workloads still do not exercise a removable same-block pair under today's analyzer output
- benchmark-shaped proof fixtures do show non-zero `Dup` / `Drop` reduction

Headline unlocked:

- Corvid now has an explicit ARC-style pair-elimination stage in the pipeline, even though the current public RC-count fixtures are not yet the workloads that expose its payoff

### Remaining close-of-17 optimization slices

| Slice | Target benchmark | Expected delta | Status |
|---|---|---|---|
| `17b-2` drop specialization | `list_heavy_strings`, verifier list-heavy path | fewer generic drop paths and fewer dynamic checks on composite teardown | pending |
| `17e` effect-typed scope reduction | allocator hot paths, verifier live-set-sensitive workloads | shorter RC-alive windows in effect-free regions | pending |
| `17b-6` effect-row-directed RC | verifier `warn/off` on tool/LLM-shaped workloads | fewer RC ops across provably safe effect boundaries | pending |
| `17b-7` latency-aware RC across tool/LLM boundaries | replay-deterministic audit workloads, verifier overhead in AI-shaped programs | move RC/GC work to compiler-invariant safepoints with better user-visible latency | pending |

## Act III - Deferred Research And Next Wave

The deferred research tier is where Corvid turns a strong memory foundation into a research-level differentiator. The goal there is not "baseline correctness plus measurement." The goal is to make Corvid measurably unusual:

- lower ownership traffic than ordinary RC systems
- low verifier cost even with stronger guarantees
- better latency shaping around tool / LLM boundaries

The next wave after the memory-foundation close is:

- native tagged-union lowering for `Result` / `Option` / `?`
- native retry runtime and codegen support

Compiled Corvid binaries still need those native backend slices before the interpreter and native tiers are feature-complete for the new `Result` / `Option` / retry surface. That is a next-wave gap, not a memory-foundation close blocker.

## Appendix - Implementation Slice Map

| Slice | Description | Status | Commit | Notes |
|---|---|---|---|---|
| `17a` | Typed heap headers + per-type typeinfo + non-atomic RC | shipped | `1fea6a0` | foundation |
| `17b-0` | Retain/release counters + baseline RC counts | shipped | `7ef4304` | measurement baseline |
| `17b-1a` | `Dup` / `Drop` IR + borrow signature scaffolding | shipped | `82f78b5` | scaffolding |
| `17b-1b.1` | Borrow inference + callee-side ABI elision | shipped | `2bce2a8` | first ownership win |
| `17b-1b.2` | String BinOp borrow-at-use-site peephole | shipped | `71c7fe4` | peephole family |
| `17b-1b.3` | `FieldAccess` / `Index` borrow-at-use-site peephole | shipped | `de3acb5` | peephole family |
| `17b-1b.4` | `for`-loop iterator borrow-at-use-site peephole | shipped | `a725449` | peephole family |
| `17b-1b.5` | Call-arg borrow-at-use-site peephole | shipped | `b0a911e` | peephole family |
| `17b-1b.6a` | CFG + liveness + ownership dataflow analysis | shipped | `760b07e` | unified-pass groundwork |
| `17b-1b.6b` | IR `Dup` / `Drop` insertion driven by `.6a` plan | shipped | `1d1af44` | unified-pass groundwork |
| `17b-1b.6c` | Wire `Dup` / `Drop` pass into codegen pipeline (opt-in) | shipped | `f3762cd` | opt-in stage |
| `17b-1b.6d-1` | Guard scattered emit sites + runtime flag | shipped | `8e2e98e` | transition stage |
| `17b-1b.6d-2a` | Entry-main + drop-before-return + BinOp consume | shipped | `520e30b` | transition stage |
| `17b-1b.6d-2` | Unified ownership pass is the default | shipped | `0cc7895` | current default |
| `17b-1c` | Whole-program retain/release pair elimination | shipped | `046806d` | Dev B |
| `17b-2` | Drop specialization | pending | `-` | Dev A next |
| `17b-3` | Reuse analysis (Perceus / Koka direction) | deferred research | `-` | research-tier |
| `17b-4` | Morphic-style alias-mode specialization | deferred research | `-` | research-tier |
| `17b-5` | Choi-style escape analysis | deferred research | `-` | research-tier |
| `17b-6` | Effect-row-directed RC | pending | `-` | Dev A, innovation moat |
| `17b-7` | Latency-aware RC across tool / LLM boundaries | pending | `-` | Dev A, innovation moat |
| `17c` | Cranelift safepoints + emitted stack-map table | shipped | `e55efea` | native GC root discovery |
| `17d` | Native mark-sweep cycle collector | shipped | `ca428bf` | native tier closes cycles |
| `17e` | Effect-typed scope reduction | pending | `-` | Dev A |
| `17f / 17f++` | Replay-deterministic GC triggers + refcount verifier | shipped | `a3b841d` | shipped as `17f++` |
| `17g` | `Weak<T>` with effect-typed invalidation | shipped | `ba01e78` | Dev B |
| `17h.1` | VM-owned heap handles | shipped | `318c892` | VM refactor |
| `17h.2` | VM Bacon-Rajan cycle collector | shipped | `91d95ac` | VM parity foundation |
| `17i` | Close-out + benchmarks + results doc | in progress | `-` | Dev B |

### Deferred Research Work

- `17b-3` reuse analysis: match drop/alloc pairs and reuse storage in place when uniqueness and size rules allow it. Deferred because the close is about the measured foundation plus first-wave optimization, not the full Perceus research tail.
- `17b-4` Morphic-style specialization: specialize call sites by alias mode to cut ownership traffic further. Deferred because it is a second-order specialization pass, not required to close the current measured baseline.
- `17b-5` Choi-style escape analysis: promote non-escaping allocations out of the heap. Deferred because it is a larger interprocedural research slice than the current close window supports.
- VM collector locality: reduce temporary buffers and traversal cost in the interpreter collector. Deferred because native tier numbers and ownership-pass work are the current close priority.

### Next Native Backend Wave

- `18d` native lowering for tagged unions and `?`: compiled binaries still need native `Result` / `Option` / `?` support to match the interpreter tier.
- `18e` retry runtime + codegen support: completes native support for `try ... retry ... backoff ...`.

This is the next focus after the memory-foundation close. It is intentionally kept out of the foundation-close scope.

## Open lock conditions

This document is not final yet. It locks only when both are true:

1. `17b-2`, `17e`, `17b-6`, and `17b-7` land and the same `memory_runtime` harness is rerun after each meaningful optimization slice
2. the final numbers are folded into the tables without hedged language
