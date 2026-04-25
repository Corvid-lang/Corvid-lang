# WASM browser approval demo

This is the Phase 23 browser smoke demo. It compiles one Corvid source file to
WASM, loads the generated ES module in a browser, supplies typed host
capabilities, displays the approval boundary, and records replay-compatible
trace events from the generated loader.

## Build

```powershell
cargo run -q -p corvid-cli -- build examples/wasm_browser_demo/src/refund_gate.cor --target=wasm
```

Generated artifacts land in `examples/wasm_browser_demo/target/wasm/`.

## Run

Serve the demo directory, then open `http://localhost:8000/web/`:

```powershell
python -m http.server 8000 -d examples/wasm_browser_demo
```

The page imports `../target/wasm/refund_gate.js`, which loads
`refund_gate.wasm` beside it. The host object implements:

- `prompts.refund_score(amount): bigint`
- `approvals.IssueRefund(amount): boolean`
- `tools.issue_refund(amount): bigint`

The trace panel shows `schema_header`, `run_started`, `llm_call/result`,
`approval_request/decision/response`, `tool_call/result`, and `run_completed`
events produced by the generated loader.

## Verify

```powershell
examples/wasm_browser_demo/verify.ps1
```

On Unix-like shells:

```sh
examples/wasm_browser_demo/verify.sh
```
