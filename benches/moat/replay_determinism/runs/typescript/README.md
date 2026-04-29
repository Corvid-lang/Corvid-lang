# TypeScript run slot — bounty open

This slot is open for a bounty submission. The contract is in
`../corvid/agent_spec.md`.

A submission lands as a complete sub-tree:

```
runs/typescript/
├── README.md                — this file (replace with a runtime description)
├── package.json             — pinned versions
├── tsconfig.json            — strict mode on
├── refund_bot.ts            — idiomatic Vercel AI SDK implementation
└── run_typescript.ts        — invokes refund_bot.ts N times, normalizes,
                                emits _summary.json
```

## What "idiomatic" means here

- Use Vercel AI SDK's typed surfaces (`generateObject`, `tool`,
  `streamObject`) with `zod` schemas — what a senior dev would
  actually reach for in 2026.
- Use the SDK's built-in tracing surface (`experimental_telemetry`
  with an OTEL exporter) or whatever stable tracing API the pinned
  version ships.
- Pin versions in `package.json`. The CI runner runs `npm ci`; no
  ambient configuration.
- Mock the model via the SDK's `MockLanguageModelV1` (or whatever the
  pinned version offers). Mock tools as plain TS functions.
- Normalize trace IDs, span IDs, and timestamps. Document the rules
  in this README and apply them in `run_typescript.ts`.
- Do NOT strip causal events. If OTEL spans carry `service.name` or
  `process.pid`, those are part of the trace surface and stay.

## Output contract

`runs/typescript/_summary.json` must have exactly the shape documented
in `runs/python/README.md` (with `"stack": "typescript"`).

## How submission lands

Same flow as the Python slot — see `runs/python/README.md`.

## Why this slot is open

OTEL spans carry random `traceId` / `spanId` per run by design. The
hypothesis going into the bounty window is that byte-stability is
well below 1.0 even with all wall-clock fields normalized, because
span identifiers reach into nested resource and attribute maps in
ways that resist clean replacement.

A submission that proves the SDK ships a stable, byte-comparable
trace artifact out of the box — with normalization rules that don't
paper over real divergence — replaces this stub and the published
headline updates.
