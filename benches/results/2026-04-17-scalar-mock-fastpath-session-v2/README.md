## Scalar-Mock Fast-Path Same-Session Ratio Archive

- Session ID: `2026-04-17-scalar-mock-fastpath-session-v2`
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

This session keeps every earlier harness and benchmark-path correction and adds
one more bridge-focused optimization pass:

- scalar prompt calls (`Int`, `Bool`, `Float`) now use a borrowed env-mock
  fast path that bypasses the generic prompt bridge when the benchmark fixtures
  already provide a direct queued mock response
- profiling guard env checks are cached, so the benchmark path no longer pays
  repeated `getenv` / `std::env::var` lookups when profiling is disabled

Those changes affect the real shipped benchmark path for offline / mock LLM
execution; they are not measurement-only corrections.

## Files

- `raw.jsonl`: canonical per-trial records
- `ratios.json`: machine-readable ratio summary with bootstrap confidence intervals
- `ratios.md`: reviewer-facing ratio tables

## Control Disclosure

The control scenario remains near zero on all three stacks, so control CV is
unstable as a primary session-quality summary.

- Corvid control: median `0.000244 ms`, IQR `[0.000000, 0.000488]`, CV `35.36%`
- Python control: median `0.001100 ms`, IQR `[0.000700, 0.001500]`, CV `69.11%`
- TypeScript control: median `0.001000 ms`, IQR `[0.000700, 0.001575]`, CV `58.91%`

Read the absolute control values first. The disclosed CV is dominated by
near-zero means and should not be treated as a standalone quality score for
the session.

## Outcome Summary

On this fixture set, Corvid improves again relative to the constant-prompt
session:

- Corvid vs Python now sits around `0.10x-0.17x`
- Corvid vs TypeScript now sits around `0.24x-0.39x`

This remains a scoped benchmark claim:

- same-session ratios only
- this host only
- these four shipped workflow fixtures only
- absolute milliseconds remain held until a verified-quiet host is available
