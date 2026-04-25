# Corvid WASM Target

Phase 23 begins with a real deployable foundation rather than a placeholder.
`corvid build --target=wasm <file>` emits four artifacts under `target/wasm/`:

- `<name>.wasm` - a valid WebAssembly module for scalar runtime-free agents.
- `<name>.js` - an ES module loader using `WebAssembly.instantiateStreaming`.
- `<name>.d.ts` - TypeScript declarations for the exported agents.
- `<name>.corvid-wasm.json` - a manifest describing exports and the current host ABI boundary.

## Current Boundary

The first slice supports agents whose parameters and return values are
`Int`, `Float`, `Bool`, or `Nothing`. Agent-to-agent calls and scalar arithmetic
lower into the module directly.

Scalar prompts, tools, and approvals lower to typed imports from the
`corvid:host` module:

- `prompt.<name>` for prompt calls.
- `tool.<name>` for tool calls.
- `approve.<Label>` for approval gates.

The generated JS loader exposes `adaptImports(host)` so browser and edge hosts
can provide `{ prompts, tools, approvals }` maps without writing raw
`WebAssembly.Imports` objects by hand.

`instantiate(host, { trace })` records Phase 21-style trace events while the
WASM module runs. `trace` may be an array, a callback, or an object with an
`events` array. The loader emits schema-v2 `schema_header`, `run_started`,
`llm_call/result`, `tool_call/result`, `approval_request/decision/response`,
and `run_completed` events for scalar host imports. BigInt values are converted
to JSON-safe numbers when possible, or strings when they exceed JavaScript's
safe integer range.

Unsupported AI-native features still fail loudly. Strings, structs,
provenance handles, stream callbacks, and replay trace recording require the
next host ABI slices. Browser and edge deployment must preserve Corvid's effect,
approval, provenance, budget, and replay contracts instead of compiling those
features away.

## Example

```corvid
agent add_one(x: Int) -> Int:
    y = x + 1
    return y
```

```text
corvid build src/math.cor --target=wasm
```

The generated TypeScript surface is:

```ts
export interface CorvidWasmModule {
  add_one(x: bigint): bigint;
}

export type CorvidWasmTraceSink =
  | Array<Record<string, unknown>>
  | ((event: Record<string, unknown>) => void)
  | { events: Array<Record<string, unknown>> };

export function instantiate(
  hostOrImports?: WebAssembly.Imports | CorvidWasmHost,
  options?: { trace?: CorvidWasmTraceSink },
): Promise<CorvidWasmModule>;
```

## Browser Demo

The committed browser smoke demo lives in
`examples/wasm_browser_demo`. It compiles `src/refund_gate.cor` to WASM,
loads the generated ES module from `target/wasm/refund_gate.js`, supplies typed
prompt/tool/approval host capabilities, displays the approval decision, and
renders the generated trace events.

```powershell
examples/wasm_browser_demo/verify.ps1
python -m http.server 8000 -d examples/wasm_browser_demo
```

Open `http://localhost:8000/web/` after the build finishes.

## Wasmtime Parity Harness

`cargo test -p corvid-codegen-wasm --test wasmtime_parity` runs generated WASM
under Wasmtime. The harness compares the current WASM-supported scalar parity
subset against the interpreter, then separately exercises typed scalar
prompt/approval/tool imports through a Wasmtime host.

The harness is intentionally fail-loud about the current boundary. Native parity
families that require strings, structs, lists, provenance handles, or streaming
callbacks cannot enter the WASM parity matrix until those ABI slices exist.
That keeps the deployment story honest: Phase 23 proves the browser/edge target
for scalar AI-native host capabilities without pretending the whole native
surface has already crossed the WASM boundary.

## Next Slice

Phase 24 starts the LSP and diagnostics track. WASM-side expansion continues
through the later ABI slices for strings, structs, provenance handles, and
streaming host callbacks.
