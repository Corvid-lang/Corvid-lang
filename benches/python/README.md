# Python benchmark runner

Stdlib-only benchmark runner for the shared AI-workflow fixtures.

- runtime style: direct Python control flow plus `time.sleep`
- no orchestration framework dependency
- emits one JSON object per trial

Run with:

```powershell
./benches/python/run.ps1 -Fixture tool_loop -Trials 3
```
