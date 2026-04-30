# Observe / eval AI helpers — `.cor` source counterparts

The four AI-helper CLI subcommands shipped in slice 40K
(`corvid observe explain`, `corvid observe cost-optimise`,
`corvid eval-drift`, `corvid eval-from-feedback`) ship in two
layers:

1. **A deterministic Rust handler** in
   `crates/corvid-cli/src/observe_helpers_cmd.rs` that produces
   the structured output (incident-explanation, cost-optimisation,
   drift-attribution, eval-fixture) by walking the lineage store.
   This is the always-available path — no LLM key required.

2. **A paired `.cor` source** under this directory (`examples/observe_helpers/`)
   documenting the `Grounded<T>`-shaped LLM-grounded version: typed
   effects, `@budget`, `cites strictly` clauses, and `Grounded<T>`
   return types.

The `.cor` programs in this directory are reference shapes, not
yet wired into a `corvid` runtime path that can `corvid run` them
end-to-end against a live LLM. They serve two purposes:

- They document the typed surface a production deployment
  would invoke (the audit's bullet "each helper is a Corvid
  program with `@budget`, typed effects, and `Grounded<T>` outputs"
  resolves to these source files).
- They will compile-check via `corvid check` as the relevant
  parser-level surfaces (`prompt … cites strictly`,
  `Grounded<T>` returns, `@budget` annotations) are already
  shipped at the syntax level.

## Files

- `observe_explain.cor` — RAG-grounded incident root cause.
- `observe_cost_optimise.cor` — generative cost-optimisation
  suggestions.
- `eval_drift.cor` — drift attribution across two trace runs.
- `eval_from_feedback.cor` — eval fixture from a "wrong answer"
  feedback record.

## Running today

The Rust handlers are reachable via the CLI:

```bash
corvid observe explain <trace-id> --trace-dir target/trace
corvid observe cost-optimise <agent> --trace-dir target/trace --top-n 5
corvid eval-drift --baseline=baseline.lineage.jsonl \
    --candidate=candidate.lineage.jsonl --explain
corvid eval-from-feedback --feedback=feedback.json \
    --trace-dir target/trace --out=fixture.eval.json
```

The output is structured JSON (the same shape the `.cor` programs
produce when wired through an LLM). Each output's `sources` field
carries the `(trace_id, span_id)` pairs the analysis consulted
— the `Grounded<T>` shape at the JSON layer.
