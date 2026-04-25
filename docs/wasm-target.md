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

export function instantiate(imports?: WebAssembly.Imports): Promise<CorvidWasmModule>;
```

## Next Slice

The next Phase 23 slice is a browser smoke demo that loads a generated module,
provides typed host capabilities, records a trace, and displays the approval
boundary in UI. Full `corvid replay` execution against WASM modules belongs in
the Wasmtime/Wasmer parity harness slice.
