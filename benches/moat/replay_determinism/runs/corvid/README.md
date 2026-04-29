# Corvid run — refund_bot deterministic re-execution

This stack invokes `cargo run -p refund_bot_demo` N times and asks
whether the resulting JSONL traces are byte-identical after the
documented normalization rules (see `../../README.md`).

The agent under test is `examples/refund_bot_demo/src/refund_bot.cor`.
Every run is invoked with the same input ticket, the same mocked
tools, and the same `MockAdapter` LLM stub.

## What the trace contains

A complete refund_bot trace has these event kinds in order:

```
schema_header        version, writer, commit_sha, ts_ms*, run_id*
seed_read            purpose=rollout_default_seed, value*, ts_ms*, run_id*
run_started          agent, args, ts_ms*, run_id*
tool_call            tool=get_order, args, ts_ms*, run_id*
tool_result          tool=get_order, result, ts_ms*, run_id*
llm_call             prompt=decide_refund, model, rendered, args, ts_ms*, run_id*
llm_result           prompt=decide_refund, model, result, ts_ms*, run_id*
approval_request     label=IssueRefund, args, ts_ms*, run_id*
approval_response    label=IssueRefund, approved, ts_ms*, run_id*
tool_call            tool=issue_refund, args, ts_ms*, run_id*
tool_result          tool=issue_refund, result, ts_ms*, run_id*
run_completed        ok, result, error, ts_ms*, run_id*
```

`*` marks fields the runner normalizes before byte-comparing.

Everything else — tool args, tool results, LLM prompts, LLM rendered
text, LLM results, approval labels and args, final result — must be
byte-identical across runs for that run to count as deterministic.

## Running

```bash
python run_corvid.py --n 20 --out _summary.json
```

The script:

1. Builds `refund_bot_demo` once (cargo build).
2. Loops N times, each iteration:
   - Invokes `cargo run -p refund_bot_demo` (release-quiet).
   - Locates the new trace file under
     `examples/refund_bot_demo/target/trace/`.
   - Loads, normalizes, stores the canonical bytes.
3. Computes pairwise byte-identity over all N(N-1)/2 pairs.
4. Writes `_summary.json` with the determinism rate and (if any)
   the first diverging pair's diff.
