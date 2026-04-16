## Orchestration Cost Investigation Session

- Source commit: `b28e012d494814141d0c7ae78717bee61e9b7bc4`
- Date: `2026-04-16`
- Host:
  - CPU: `Intel(R) Core(TM) Ultra 7 155U`
  - OS: `Microsoft Windows 11 Business`
  - Version: `10.0.26200`
  - Build: `26200`
  - BIOS: `1.20.0`

### Protocol

- Runner: same-session interleaving via `python benches/analysis/session.py`
- Warm-up: `3` discarded trials per stack, per scenario
- Measured trials: `30` per stack, per scenario
- Ordering: interleaved by trial index inside each scenario (`Corvid`, `Python`, `TypeScript`)
- Profiling mode: enabled
- Output files:
  - `raw.jsonl`: per-trial raw measurements
  - `ratios.json` / `ratios.md`: same-session comparative ratios
  - `investigation.json` / `investigation.md`: investigation summary

### Commands

```powershell
python benches/analysis/session.py --output-dir benches/results/2026-04-16-perf-investigation --warmup 3 --trials 30 --profile
python benches/analysis/aggregate.py benches/results/2026-04-16-perf-investigation/raw.jsonl benches/results/2026-04-16-perf-investigation/ratios.json benches/results/2026-04-16-perf-investigation/ratios.md
python benches/analysis/investigate.py benches/results/2026-04-16-perf-investigation/raw.jsonl benches/results/2026-04-16-perf-investigation/investigation.json benches/results/2026-04-16-perf-investigation/investigation.md
```

### Noise Disclosure

- Worst control-path coefficient of variation across stacks: `32.65%`
- Corvid control-path median orchestration cost: `42.810 ms`

This session is investigation data, not a clean-host absolute calibration run. The archived values are used to rank contributors to the observed orchestration gap and to preserve the same-session comparative ratios.
