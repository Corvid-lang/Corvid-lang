## Corrected Same-Session Ratio Archive

- Session ID: `2026-04-17-corrected-session`
- Source commit: `2a55ef5bb963116a4c3c5b261fb0995bd42d4cee`
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

This session corrects two measured harness artifacts from the historical close-out session:

1. Corvid now executes multiple logical trials inside one native process instead of paying binary startup on every measured trial.
2. All three stacks now subtract **measured** external wait instead of nominal fixture wait.

## Files

- `raw.jsonl`: canonical per-trial records
- `ratios.json`: machine-readable ratio summary with bootstrap confidence intervals
- `ratios.md`: reviewer-facing ratio tables

## Control Disclosure

The control scenario is now close to zero on all three stacks, which is the intended outcome of the persistent-process correction. That makes coefficient-of-variation unstable, especially for Corvid, because the mean is so small.

- Corvid control: median `0.1017 ms`, IQR `[0.0919, 0.1149]`, CV `216.89%`
- Python control: median `0.00135 ms`, IQR `[0.00095, 0.00177]`, CV `38.43%`
- TypeScript control: median `0.00080 ms`, IQR `[0.00060, 0.00147]`, CV `79.29%`

Read the absolute control values first. The CV is disclosed for completeness, but it is not a stable noise-floor summary once the control mean approaches zero.

## Outcome Summary

The same methodology correction moved the comparative story in opposite directions:

- Corvid vs Python improved materially: the historical `25x-36x` gap narrows to roughly `3x-4x`.
- Corvid vs TypeScript worsened materially: the historical `1.7x-2.6x` gap widens to roughly `8x-10x`.

Both changes come from the same correction:

- removing repeated native startup from Corvid's measured trials
- stopping the benchmark from charging Node's sleep overshoot to orchestration cost

This archive therefore supersedes the historical session for interpretation, but the historical session remains published unchanged for reproducibility.
