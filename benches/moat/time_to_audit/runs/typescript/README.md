# TypeScript run slot — bounty open

This slot is open for a bounty submission. Contract is in
`benches/moat/replay_determinism/runs/corvid/agent_spec.md` (agent
shape — same refund_bot both benchmarks target) plus the audit
questions under `../../audit_questions/`.

A submission lands as a complete sub-tree:

```
runs/typescript/
├── README.md                — this file (replace with a runtime description)
├── package.json             — pinned versions
├── tsconfig.json            — strict mode on
└── queries/
    ├── q01.ts               — answers Q01 against the TypeScript-stack corpus
    ├── q02.ts               — answers Q02
    └── q03.ts               — answers Q03
```

Plus alongside it:

```
corpus/typescript/
├── …                        — equivalent agent traces in the
                                stack-native format (Vercel AI SDK
                                experimental_telemetry OTEL export,
                                or whatever the pinned version
                                ships)
```

## What "idiomatic" means here

- Use whatever query API the stack documents as the audit interface
  — most likely an OTEL span exporter + a span-store query layer
  (jaeger-query, tempo, signoz-query, etc.).
- Pin all versions in `package.json`. CI runs `npm ci`; nothing else
  is on the path.
- The corpus must come from running the actual agent in the stack —
  not hand-crafted JSON. A bounty PR includes a `corpusGen.ts`
  alongside the queries.
- Each query takes `--corpus <dir>` and `--out <path>` and writes
  JSON. Same CLI shape as the Corvid queries.
- LOC is counted by the runner; the bounded `// region: cli-plumbing`
  ... `// endregion` block is excluded.

## How submission lands

Same flow as the Python slot — see `runs/python/README.md`.

## Why this slot is open

The hypothesis: OTEL spans force an audit query to traverse a span
tree, follow parent/child links, and re-derive causal structure
that a JSONL agent trace exposes directly. That's more LOC per
question. A submission proving the opposite — with the SDK's
out-of-the-box trace artifact and minimal normalization — replaces
this stub and the published headline updates.
