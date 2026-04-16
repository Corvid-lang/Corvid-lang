# Corvid native benchmark runner

This runner executes the shared AI-workflow fixtures through native Corvid
binaries, not the interpreter.

## What it measures

- compiled Corvid program execution
- native ownership/refcount path
- prompt-boundary RC optimizations currently enabled in the native tier

## Mocking model

- prompt replies come from `CORVID_TEST_MOCK_LLM_REPLIES`
- prompt latencies come from `CORVID_TEST_MOCK_LLM_LATENCY_MS`
- tool replies come from the `corvid_bench_tools` staticlib via
  `CORVID_BENCH_TOOL_RESPONSES`
- tool latencies come from `CORVID_BENCH_TOOL_LATENCIES_MS`

Each trial writes one JSON object per line with:

- `total_wall_ms`
- `external_wait_ms`
- `orchestration_overhead_ms`
- trace size fields

## Run

```powershell
./benches/corvid/run.ps1 -Fixture tool_loop -Trials 3
```

## Notes

- the runner builds a benchmark-only `#[tool]` staticlib under
  `benches/corvid/tools`
- workloads return stringified JSON payloads so the native tool ABI can stay
  within the current scalar boundary
- `retry_workflow` currently models prompt + repeated tool attempts natively;
  fixed retry sleeps are injected by the runner into the external-wait ledger
