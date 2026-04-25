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

The next Phase 23 slice is replay-compatible host tracing: JS-side prompt,
tool, and approval imports must write Phase 21 trace events so native-captured
and WASM-captured runs share one replay format.
