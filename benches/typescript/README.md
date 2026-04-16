# TypeScript benchmark runner

Node-based benchmark runner for the shared AI-workflow fixtures.

- runtime style: direct async/await
- no orchestration framework dependency
- emits one JSON object per trial

Run with:

```powershell
./benches/typescript/run.ps1 -Fixture tool_loop -Trials 3
```
