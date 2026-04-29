# Corvid run — audit queries against the JSONL trace

The Corvid runtime writes a schema-versioned JSONL trace under
`target/trace/run-<ts>.jsonl`. Each event is a single JSON line with
a stable `kind` discriminator (`schema_header`, `seed_read`,
`run_started`, `tool_call`, `tool_result`, `llm_call`, `llm_result`,
`approval_request`, `approval_decision`, `approval_response`,
`approval_token_issued`, `host_event`, `run_completed`).

That schema is the canonical audit surface. Every query under
`queries/` opens the corpus directory, parses each `.jsonl` file
line-by-line, and walks the events directly.

## What the queries answer

- `q01.py` — list every refund issued (Q01)
- `q02.py` — refunds where amount > $50 (Q02)
- `q03.py` — count refunds per user (Q03)

Each query takes `--corpus <dir>` and `--out <path>` and writes a
JSON answer that the runner compares byte-for-byte against
`audit_questions/<NN>-<slug>/expected_answer.json`.

## How to run a single query

```bash
python benches/moat/time_to_audit/runs/corvid/queries/q01.py \
    --corpus benches/moat/time_to_audit/corpus/corvid \
    --out    /tmp/q01.json
```

## How to regenerate the corpus

```bash
cargo run -q -p refund_bot_corpus_gen -- \
    --out benches/moat/time_to_audit/corpus/corvid
```

Re-running `corpus_gen` writes the same set of `run-NN.jsonl`
filenames (sequence-numbered, not timestamp-keyed) so the committed
corpus stays reproducible.
