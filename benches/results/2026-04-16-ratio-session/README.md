## Same-Session Ratio Archive

- Session ID: `2026-04-16-ratio-session`
- Source commit: `6f47984c0a164df2d0d953899c9ef9e3e2361dfd`
- Host:
  - `Dell Latitude 5550`
  - `Intel(R) Core(TM) Ultra 7 155U` (`12` cores / `14` logical processors)
  - `16,597,598,208` bytes RAM
  - `Microsoft Windows 11 Business` `10.0.26200` (build `26200`)

## Protocol

- Warm-up: `3` discarded trials per stack per scenario
- Measured: `30` trials per stack per scenario
- Interleaving: `Corvid -> Python -> TypeScript` repeated by `trial_idx`
- Published metric: `orchestration_ms = wall_ms - external_wait_ms`
- Publication format: ratios only; absolute milliseconds remain held

## Files

- `raw.jsonl`: canonical per-trial records
- `ratios.json`: machine-readable ratio summary with bootstrap confidence intervals
- `ratios.md`: reviewer-facing ratio tables

## Noise Disclosure

- Control scenario disclosed host noise floor: `41.40%` coefficient of variation on the worst stack
- Per-stack control CV:
  - `corvid`: `20.90%`
  - `python`: `41.40%`
  - `typescript`: `28.69%`

This archive is published because the same-session ratio methodology keeps the three stacks under the same host drift. The noise disclosure stays with the archive so readers can judge whether the ratio spread is meaningful on this host.
