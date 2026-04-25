# Corvid WASM Target

Phase 23 begins with a real deployable foundation rather than a placeholder.
`corvid build --target=wasm <file>` emits four artifacts under `target/wasm/`:

- `<name>.wasm` - a valid WebAssembly module for scalar runtime-free agents.
- `<name>.js` - an ES module loader using `WebAssembly.instantiateStreaming`.
- `<name>.d.ts` - TypeScript declarations for the exported agents.
- `<name>.corvid-wasm.json` - a manifest describing exports and the current host ABI boundary.

## Current Boundary

The first slice supports agents whose parameters and return values are
`Int`, `Float`, `Bool`, or `Nothing`, and whose body does not call prompts,
tools, approvals, or other runtime host capabilities. Agent-to-agent calls and
scalar arithmetic lower into the module directly.

Unsupported AI-native features fail loudly. A prompt call reports that the
Phase 23 host LLM ABI is required; a tool call reports that the host tool ABI is
required; an approval reports that the host approval ABI is required. This is
intentional. Browser and edge deployment must preserve Corvid's effect,
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

The next Phase 23 slice is the host-capability ABI. That is where prompts,
tools, approvals, replay recording, and provenance handles become browser/edge
imports with the same contracts that native and FFI targets already expose.
