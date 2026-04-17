## Low-Overhead Same-Session Ratio Archive

- Session ID: `2026-04-17-direct-counter-session`
- Source commit: `f36d6c2`
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
- Published metric: `orchestration_ms = wall_ms - actual_external_wait_ms`
- Publication format: ratios only; absolute milliseconds remain held

This session keeps the persistent-process and actual-wait corrections from the prior corrected session and removes one more Corvid-only benchmark artifact:

- prompt and tool wait accounting now comes from direct per-trial native counters in the benchmark summary
- ordinary measured runs no longer emit per-wait profiling JSON to stderr

That change does not alter the metric. It removes serialization and parsing overhead that only the Corvid runner had been paying.

## Files

- `raw.jsonl`: canonical per-trial records
- `ratios.json`: machine-readable ratio summary with bootstrap confidence intervals
- `ratios.md`: reviewer-facing ratio tables

## Control Disclosure

The control scenario remains close to zero on all three stacks, so control CV is still unstable as a primary noise summary.

- Corvid control: median `0.11265 ms`, IQR `[0.08502, 0.12668]`, CV `46.41%`
- Python control: median `0.00130 ms`, IQR `[0.00110, 0.00150]`, CV `25.60%`
- TypeScript control: median `0.00125 ms`, IQR `[0.00103, 0.00213]`, CV `65.58%`

Read the absolute control values first. The CV is disclosed for completeness, but it is not the main control signal once the medians are this small.

## Outcome Summary

Relative to the previous corrected harness session:

- Corvid vs Python improved again, from roughly `3x-4x` slower to roughly `1.2x-1.8x` slower
- Corvid vs TypeScript improved again, from roughly `7.7x-10.2x` slower to roughly `2.4x-4.9x` slower

This session still does **not** support a claim that Corvid is faster than either stack. It does support a much stronger developer-facing statement: once harness artifacts are removed, Corvid's orchestration overhead is in the same order of magnitude as Python and within a few multiples of Node on these workflows.
