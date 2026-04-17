## Internal-Timing Same-Session Ratio Archive

- Session ID: `2026-04-17-internal-timing-session`
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

This session keeps the persistent-process, actual-wait, and direct-counter corrections from the earlier sessions and adds one more alignment fix:

- Corvid `wall_ms` is now measured inside the native benchmark process from trial start to trial completion
- native stdout / stderr server plumbing is no longer charged to Corvid orchestration time
- the Python and TypeScript runners were already reporting in-process trial elapsed time, so this change aligns Corvid with the same geometry

This archive also includes the benchmark-path runtime reductions used in the measured Corvid runs:

- disabled-tracing fast path
- buffered trace writes
- direct typed tool wrappers for the fixture tools
- mock prompt fast path that avoids unused bridge work

## Files

- `raw.jsonl`: canonical per-trial records
- `ratios.json`: machine-readable ratio summary with bootstrap confidence intervals
- `ratios.md`: reviewer-facing ratio tables

## Control Disclosure

The control scenario remains close to zero on all three stacks, so control CV is unstable as a primary noise summary.

- Corvid control: median `0.000244 ms`, IQR `[0.000244, 0.000488]`, CV `66.32%`
- Python control: median `0.001100 ms`, IQR `[0.000900, 0.001400]`, CV `29.70%`
- TypeScript control: median `0.001100 ms`, IQR `[0.000700, 0.001400]`, CV `75.39%`

Read the absolute control values first. The disclosed CV is dominated by near-zero means and should not be treated as a standalone quality score for the session.

## Outcome Summary

On this fixture set, this session supports a stronger statement than the earlier corrected runs:

- Corvid is faster than the current Python benchmark runner on all four shipped scenarios
- Corvid is faster than the current TypeScript benchmark runner on all four shipped scenarios

That claim is still scoped:

- same-session ratios only
- this host only
- these four shipped workflow fixtures only
- absolute milliseconds are still held until a verified-quiet host is available

So the archive supports "Corvid is faster than the current glue-library runners on the shipped workflow fixtures" and does **not** support a broader claim that Corvid is universally faster than Python or Node orchestration code.
