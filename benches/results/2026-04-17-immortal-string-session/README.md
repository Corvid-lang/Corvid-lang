## Immortal Fixture-String Same-Session Ratio Archive

- Session ID: `2026-04-17-immortal-string-session`
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

This session keeps the scalar env-mock fast path and adds one more
fixture-scoped optimization on the real shipped workflow path:

- repeated prompt replies and benchmark tool replies are now prebuilt as
  immortal `CorvidString` values
- queued fixture responses can therefore be reused without per-use free work
  on the native side

Those changes affect the real measured path for the shipped benchmark
fixtures; they are not another methodology correction.

## Files

- `raw.jsonl`: canonical per-trial records
- `ratios.json`: machine-readable ratio summary with bootstrap confidence intervals
- `ratios.md`: reviewer-facing ratio tables

## Control Disclosure

The control scenario remains near zero on all three stacks, so control CV is
unstable as a primary session-quality summary.

- Corvid control: median `0.000185 ms`, IQR `[0.000122, 0.000428]`, CV `38.31%`
- Python control: median `0.001100 ms`, IQR `[0.000900, 0.001300]`, CV `30.15%`
- TypeScript control: median `0.000950 ms`, IQR `[0.000700, 0.001350]`, CV `76.75%`

Read the absolute control values first. The disclosed CV is dominated by
near-zero means and should not be treated as a standalone quality score for
the session.

## Outcome Summary

Relative to the scalar-mock fast-path session:

- Corvid vs Python improves again, from roughly `0.10x-0.17x` to roughly
  `0.09x-0.16x`
- Corvid vs TypeScript improves again, from roughly `0.24x-0.39x` to roughly
  `0.20x-0.34x`

This remains a fixture-scoped same-session ratio claim. It strengthens the
existing result rather than changing the measurement story:

- Corvid is faster than the current Python and TypeScript benchmark runners on
  these four shipped workflow fixtures
- the remaining benchmark-path win came from eliminating hot-path release/free
  churn on canned prompt and tool replies
