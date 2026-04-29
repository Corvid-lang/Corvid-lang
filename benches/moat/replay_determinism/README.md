# Benchmark — replay determinism rate

For each agent under `runs/<stack>/`, the runner asks: when the same
agent is executed N times with identical inputs, what fraction of runs
produce byte-identical causal traces (after normalizing wall-clock
fields)?

Corvid's `corvid-vm` writes a schema-versioned JSONL trace by
construction. Every causal event — tool calls, tool results, LLM
prompts, LLM results, approval requests/responses, run start/complete —
lands in the trace with a stable schema. Two runs of the same program
with the same inputs and mocked external surfaces produce the same
trace bytes after normalizing `ts_ms`, `run_id`, and the
`rollout_default_seed` value.

LangChain (Python) and Vercel AI SDK (TypeScript) have no built-in
schema-versioned causal trace. Tracing via LangSmith or
OpenTelemetry adds spans with random UUIDs, attempt counters, and
provider-internal metadata that vary per run by design.

The headline number is the determinism rate: **of N runs against
identical inputs, how many produce byte-identical normalized
traces?**

## What counts as deterministic re-execution

| Stack | Counts as deterministic if... |
|---|---|
| **Corvid** | After normalizing `ts_ms`, `run_id`, and the `rollout_default_seed` value, the JSONL trace file is byte-identical across all N runs. |
| **Python + LangChain (LangSmith)** | After normalizing wall-clock and span-UUID fields, the persisted trace artifact is byte-identical across all N runs. |
| **TypeScript + Vercel AI SDK (OTEL)** | After normalizing trace IDs, span IDs, and timestamps, the OTEL export is byte-identical across all N runs. |

A trace whose causal *structure* matches but whose *bytes* don't (after
the documented normalization) does NOT count. The point is whether a
downstream auditor can compare two runs by `diff` without writing a
custom semantic comparator.

## Normalization rules (Corvid)

The runner replaces these per-execution fields with sentinel tokens
before diffing — they're either wall-clock-derived or fresh-randomness
by design (a one-time approval token's `token_id` MUST be unique per
issuance; that's the security model, not flakiness):

- `ts_ms`, plus any field whose name ends in `_at_ms` (e.g.
  `issued_at_ms`, `expires_at_ms`) → `<TS>`
- `run_id` → `<RUN_ID>`
- `token_id` → `<TOKEN_ID>` (fresh random per approval token)
- On `seed_read` events with `purpose = "rollout_default_seed"`, the
  `value` field → `<SEED>`

The runner does NOT normalize:

- Tool call arguments / results
- LLM prompt / rendered text / result
- Approval labels / args / approved-bool
- Final `run_completed.result`
- Schema-header `version`, `writer`, `commit_sha`, `source_path`
- `host_event` payloads (e.g. `llm.usage` accounting)

If any of those vary across runs, the agent is non-deterministic and
the determinism rate drops. That's the signal we want.

## Run layout

```
runs/
├── corvid/
│   ├── README.md         — what this stack runs
│   └── run_corvid.py     — invokes `cargo run -p refund_bot_demo` N times
├── python/
│   └── README.md         — bounty-open: idiomatic LangChain equivalent wanted
└── typescript/
    └── README.md         — bounty-open: idiomatic Vercel AI SDK equivalent wanted
```

Each stack's runner emits a small JSON summary
(`runs/<stack>/_summary.json`) with shape:

```json
{
  "stack": "corvid",
  "n": 20,
  "byte_identical_pairs": 190,
  "total_pairs": 190,
  "determinism_rate": 1.0,
  "first_diverging_pair": null
}
```

The top-level orchestrator (`runner/run.py`) reads each stack's
summary, composes `RESULTS.md`, and the CI drift gate diffs the
regenerated file against the committed one.

## How to run locally

```bash
# Corvid only (fast, no external deps beyond cargo):
python benches/moat/replay_determinism/runs/corvid/run_corvid.py \
    --n 20 \
    --out benches/moat/replay_determinism/runs/corvid/_summary.json

# Compose the headline:
python benches/moat/replay_determinism/runner/run.py \
    --runs-dir benches/moat/replay_determinism/runs \
    --out      benches/moat/replay_determinism/RESULTS.md
```

## Bounty status (opened 2026-04-29)

The Python and TypeScript run slots are open for bounty submissions.

A submission counts if it:

1. Provides an idiomatic Python or TypeScript implementation of the
   same agent (see `runs/corvid/agent_spec.md` for the contract:
   inputs, expected tool calls, expected final return shape).
2. Lands a `run_<stack>.py` (or `.ts` / `.js`) under
   `runs/<stack>/` that runs the agent N times, captures whatever
   trace artifact the stack natively emits, applies the documented
   normalization for that stack, and writes `_summary.json`.
3. Uses libraries / patterns a senior dev would *actually* reach for.
   Custom span exporters, hand-rolled trace formatters, or
   normalization rules that paper over real divergence are
   rejected — the point is what the stack ships out of the box.

The runner re-classifies that stack's column as soon as the
submission lands. If the submission's determinism rate is higher
than expected (e.g. LangSmith turns out to be byte-stable with a
particular config), the published numbers update accordingly.

## Honesty rules

1. **Mocked external surfaces.** All stacks must mock the LLM and
   external tool calls so the test isolates the *trace surface*, not
   the underlying providers. A real OpenAI call is non-deterministic
   by definition; that's not what we're measuring.
2. **No "winning by under-recording."** The Corvid trace captures
   every causal event the agent emits. A baseline that records less
   information per run will trivially be more byte-stable; submissions
   that strip events to manufacture stability are rejected.
3. **Adversarial review.** Submissions sit under
   `docs/effects-spec/bounty.md` review for ≥7 days before publishing;
   if a reviewer submits a config that improves a baseline's
   determinism rate, the rewrite replaces the published version and
   the score updates accordingly.

## Path to first headline

Initial commit ships:

- Corvid run with N=20 against `examples/refund_bot_demo`.
- Python and TypeScript slots stubbed with bounty-open READMEs.
- Drift-gated CI job over the Corvid column.

The publishable headline waits for at least one Python or TypeScript
baseline to land. Until then, `RESULTS.md` reports partial coverage
explicitly.
