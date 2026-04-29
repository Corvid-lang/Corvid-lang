# Benchmark — time-to-audit

For each audit question under `audit_questions/`, the runner asks:
how many lines of code does it take to extract a typed, structured
answer from the canonical trace surface the language ships, and how
long does the query take to run against a fixed corpus?

Corvid's `corvid-vm` writes a schema-versioned JSONL trace by
construction. Every causal event — tool calls, tool results, LLM
prompts, LLM results, approval requests, approval responses,
approval-token issuance — lives at a stable JSON path. A regulator
asking "list every refund issued for >$50 with the approving LLM's
rationale" answers themselves with a one-screen Python script over
the trace directory.

LangChain (Python) and Vercel AI SDK (TypeScript) have no built-in
schema-versioned causal trace. The audit query has to either go
through LangSmith's cloud API, query an OTEL span backend, or
re-derive structure from log lines. That's not a Corvid attack —
it's the consequence of tracing being optional / cloud / vendored
in those stacks.

The headline numbers are:

- **Lines of code** to answer each audit question against the
  canonical trace surface (lower is better).
- **Wall-clock seconds** to run the query against a fixed corpus
  (lower is better, but secondary at this corpus size).

## Corpus

`corpus/corvid/` holds JSONL traces produced by running
`refund_bot` against a varied set of tickets. They're real
runtime output (not hand-rolled), generated once via the
`refund_bot_corpus_gen` workspace binary:

```bash
cargo run -q -p refund_bot_corpus_gen -- \
    --out benches/moat/time_to_audit/corpus/corvid
```

The committed corpus is the artifact the audit queries run
against. Re-running `corpus_gen` yields traces that differ only in
the normalized fields (`ts_ms`, `run_id`, `token_id`, `_at_ms`) — the
audit queries strip those before comparing answers, so the
benchmark is reproducible.

`corpus/python/` and `corpus/typescript/` are bounty-open. A
bounty submission lands an equivalent corpus in the stack's
canonical trace format (LangSmith JSON export, OTEL span export,
or whatever the *actually shipped* tracing surface is in the
pinned library version) plus its query implementations. The
underlying agent contract is shared with the
`benches/moat/replay_determinism/` benchmark and lives in
`benches/moat/replay_determinism/runs/corvid/agent_spec.md`.

## Audit questions

`audit_questions/<NN>-<slug>/` contains:

- `question.md` — the question in plain English plus the expected
  answer schema.
- `expected_answer.json` — the typed answer that any correct query
  must produce.

The runner verifies each stack's query against
`expected_answer.json` byte-for-byte (after sorting keys). A query
that produces a different answer fails the benchmark for that
question.

## Per-stack queries

```
runs/
├── corvid/
│   ├── README.md
│   └── queries/
│       ├── q01.py    — answers question 01
│       ├── q02.py    — answers question 02
│       └── q03.py    — answers question 03
├── python/
│   └── README.md     — bounty-open
└── typescript/
    └── README.md     — bounty-open
```

Each query is invoked as:

```bash
python runs/<stack>/queries/q<NN>.py \
    --corpus benches/moat/time_to_audit/corpus/<stack> \
    --out    /tmp/q<NN>.json
```

The query reads the corpus, computes the answer, writes JSON to
`--out`. The runner then byte-compares the output against
`expected_answer.json`.

## What gets counted as "lines of code"

The LOC counter (in `runner/run.py`) excludes:

- Blank lines
- Lines that are *only* a comment (`#`-prefix in Python, `//`-prefix
  in JS/TS)
- The `argparse` / CLI plumbing inside a clearly-marked
  `# region: cli-plumbing` ... `# endregion` block (so the published
  metric reflects audit-logic LOC, not boilerplate)

It includes everything else, including imports. An honest baseline
that needs `from langchain_smith import ...` pays for that line, the
same way a Corvid baseline pays for `import json`.

## How submission lands

Same flow as `replay_determinism` and `provenance_preservation`:

1. Bounty submitter lands a `runs/<stack>/` sub-tree with pinned
   versions, real implementations, and corpus fixtures matching
   the agent contract in `runs/corvid/agent_spec.md`.
2. Submitter regenerates RESULTS.md locally; CI drift gate diffs
   against the committed file.
3. ≥7-day adversarial review window per
   `docs/effects-spec/bounty.md` before the published numbers
   update.

## Honesty rules

1. **Real implementations, not strawmen.** A Python answer using
   `dict[str, object]` and naked string parsing of LangSmith log
   files is rejected — the right baseline is whatever query API
   the stack documents as the audit interface (LangSmith Run API,
   OTEL span exporter + jaeger-query, etc.).
2. **No "winning by hand-rolling structure."** The Corvid query
   uses the canonical JSONL trace, not a synthesized custom
   format.
3. **Wall-clock is informational, not load-bearing.** At this
   corpus size, all stacks run in <1s. The point is that audit
   logic exists and is short, not that it's microsecond-fast.
4. **Same audit questions across stacks.** A submission that
   answers a different question doesn't count.

## Path to first headline

Initial commit ships:

- 3 audit questions covering: list-by-event-kind, filter-by-amount,
  group-by-user.
- Corvid corpus + Corvid answer scripts for all 3.
- Python and TypeScript stack stubs with bounty-open READMEs.
- Drift-gated CI job.

The publishable headline waits for at least one Python or TypeScript
baseline to land. Until then, `RESULTS.md` reports partial coverage
explicitly.
