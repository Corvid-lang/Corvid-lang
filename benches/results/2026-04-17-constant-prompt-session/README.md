## Constant-Prompt Same-Session Ratio Archive

- Session ID: `2026-04-17-constant-prompt-session`
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

This session keeps the earlier internal native timing alignment and adds one
more runtime/codegen reduction:

- prompt templates whose interpolated arguments are compile-time
  string / int / bool literals are now rendered to one immortal string literal
  during native lowering instead of being rebuilt via runtime stringify +
  concat work on every prompt call

That optimization is general native-code behavior, not a benchmark-only branch.

## Files

- `raw.jsonl`: canonical per-trial records
- `ratios.json`: machine-readable ratio summary with bootstrap confidence intervals
- `ratios.md`: reviewer-facing ratio tables

## Control Disclosure

The control scenario remains close to zero on all three stacks, so control CV is unstable as a primary noise summary.

- Corvid control: median `0.000488 ms`, IQR `[0.000244, 0.000732]`, CV `37.34%`
- Python control: median `0.001100 ms`, IQR `[0.000700, 0.001600]`, CV `53.56%`
- TypeScript control: median `0.001000 ms`, IQR `[0.000900, 0.001800]`, CV `75.68%`

Read the absolute control values first. The disclosed CV is dominated by near-zero means and should not be treated as a standalone quality score for the session.

## Outcome Summary

On this fixture set, Corvid remains ahead of both comparison stacks and improves again relative to the prior internal-timing session:

- Corvid vs Python now sits around `0.17x-0.29x`
- Corvid vs TypeScript now sits around `0.37x-0.61x`

This claim stays scoped:

- same-session ratios only
- this host only
- these four shipped workflow fixtures only
- absolute milliseconds are still held until a verified-quiet host is available
