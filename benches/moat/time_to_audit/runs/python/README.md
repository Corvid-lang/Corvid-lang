# Python run slot — bounty open

This slot is open for a bounty submission. The contract is in
`benches/moat/replay_determinism/runs/corvid/agent_spec.md` (for the
underlying refund_bot agent — the same agent both benchmarks
target) plus the audit questions under `../../audit_questions/`.

A submission lands as a complete sub-tree:

```
runs/python/
├── README.md                — this file (replace with a runtime description)
├── requirements.txt         — pinned versions
└── queries/
    ├── q01.py               — answers Q01 against the Python-stack corpus
    ├── q02.py               — answers Q02
    └── q03.py               — answers Q03
```

Plus, alongside it:

```
corpus/python/
├── …                        — equivalent agent traces in the
                                stack-native format (LangSmith JSON
                                export, OTEL JSON export, structured
                                JSON logs from the pinned tracing
                                library, etc.)
```

## What "idiomatic" means here

- Use whatever query / read API the stack documents as the audit
  interface — `langsmith.Client.list_runs(...)`, structured-log
  parsing if that's what the pinned tracing library actually emits,
  or an OTEL span store query layer.
- Pin all versions in `requirements.txt`. CI installs from that file
  before running queries; nothing else is on the path.
- The corpus must be produced by *running the agent* in the stack —
  not hand-rolled JSON. A bounty PR includes a `corpus_gen.py`
  alongside the queries.
- Each query reads `--corpus <dir>` and writes its answer to
  `--out <path>` as JSON. Same CLI shape as the Corvid queries.
- The query's audit-logic LOC is counted by the runner; the bounded
  `# region: cli-plumbing` ... `# endregion` block is excluded.

## How submission lands

Same flow as `replay_determinism` and `provenance_preservation`:

1. Open a PR with the full sub-tree (queries + corpus + corpus_gen).
2. Regenerate `RESULTS.md` locally; CI drift gate must pass.
3. Note the PR in `docs/effects-spec/bounty.md`.
4. ≥7-day adversarial review window before published numbers update.

The reviewer verifies:

- Versions pinned, no ambient configuration.
- Corpus actually came from running the agent (not hand-rolled).
- Each query answers the question correctly (runner compares bytes).
- LOC counts are honest (no hidden helper modules that don't get
  counted).

## Why this slot is open

The hypothesis going into the bounty window is that LangSmith's Run
API or OTEL span queries take meaningfully more LOC per question
than the Corvid JSONL walk — because the LangChain / Vercel stacks
don't emit a stable, agent-causal trace surface by construction.
Audit logic in those stacks usually means joining tracing data with
out-of-band tool-call metadata.

A submission proving the opposite — with idiomatic, pinned-version
implementations — replaces this stub and the published headline
updates.
