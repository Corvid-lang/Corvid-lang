# RC/GC Tuning Assessment Session

- Host CPU: `Intel(R) Core(TM) Ultra 7 155U`
- Host OS: `Microsoft Windows 11 Business 10.0.26200 (build 26200)`
- Runner: `benches/corvid/stress_runner`
- Build: `cargo build --release --manifest-path benches/corvid/stress_runner/Cargo.toml`
- Trials: `30` per configuration

## Scope

This archive answers one question: **at what allocation pressure do Corvid's
refcount and native cycle collector become material costs?**

It is Corvid-only. There is no Python or TypeScript comparison in this slice.

## Workloads

The session covers three synthetic stress surfaces:

1. `allocation_scaling`
   - one explicit rooted GC at the end of each trial
   - release scales: `19`, `100`, `1000`, `10000`, `100000`
2. `gc_trigger_sensitivity`
   - immediate alloc/release loop at the highest scale (`100000`)
   - explicit GC cadence every `100`, `1000`, `10000`, `50000`, or disabled
3. `cycle_stress`
   - `N` mutual-reference pairs (`A -> B -> A`) per trial
   - pair scales: `10`, `100`, `1000`, `10000`

## Important Methodology Note

`gc_trigger_sensitivity` uses an **explicit GC cadence** (`corvid_gc_from_roots`
every `N` allocations) rather than the runtime's automatic trigger inside
`corvid_alloc_typed`.

Why:

- the stress harness is a direct Rust FFI caller, not a compiled Corvid program
- automatic GC fires *inside* `corvid_alloc_typed`
- at that point the in-flight allocation is not yet represented by Corvid stack
  map roots
- using the raw auto-trigger path from this harness would therefore measure a
  harness/rooting artifact, not a trustworthy threshold curve

This session is measuring **collector cadence sensitivity**, not validating the
raw FFI auto-trigger path.

## Realistic Context

Current shipped workflow fixtures sit around `12-19` releases per trial.

A rough heavy v0.1 orchestration estimate is:

- `50` logical steps
- about `100` short-lived heap releases per step
- roughly `5000` releases per trial

So the `100000` release point in this archive is:

- about `20x` that rough heavy-workflow estimate
- about `5000x` the smallest shipped workflow shape

That makes the top end a real stress test rather than a realistic steady-state
v0.1 workload.

## Files

- `raw.jsonl`: per-trial raw records
- `summary.json`: computed medians
- `summary.md`: rendered markdown tables used in the investigation doc
