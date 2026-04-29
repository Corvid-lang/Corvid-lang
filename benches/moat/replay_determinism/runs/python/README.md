# Python run slot — bounty open

This slot is open for a bounty submission. The contract is in
`../corvid/agent_spec.md`.

A submission lands as a complete sub-tree:

```
runs/python/
├── README.md                — this file (replace with a runtime description)
├── requirements.txt         — pinned versions
├── refund_bot.py            — idiomatic LangChain implementation
└── run_python.py            — invokes refund_bot.py N times, normalizes,
                                emits _summary.json
```

## What "idiomatic" means here

- Use LangChain's typed surfaces where they exist
  (`langchain.agents.AgentExecutor`, `LLMChain`, structured tools).
- Use LangSmith's local trace export (or whatever the *actually shipped*
  tracing surface is in the LangChain version you pin).
- Pin versions in `requirements.txt`. The CI runner installs from that
  file; nothing else is on the path.
- Mock the LLM with LangChain's `FakeListLLM` (or equivalent for the
  pinned version). Mock the tools as plain Python callables. The
  agent's external surfaces must be deterministic by construction.
- Normalize wall-clock and trace-identifier fields (UUIDs, span IDs,
  `start_time` / `end_time`, run UUIDs). Document the normalization
  rules in this README and apply them in `run_python.py`.
- Do NOT strip causal events to manufacture stability. If LangSmith
  emits per-attempt retry IDs, those count as part of the trace.

## Output contract

`runs/python/_summary.json` must have exactly this shape:

```json
{
  "stack": "python",
  "n": 20,
  "byte_identical_pairs": 190,
  "total_pairs": 190,
  "determinism_rate": 1.0,
  "first_diverging_pair": null
}
```

`first_diverging_pair` is either `null` (all runs match) or:

```json
{
  "run_a": "<run-id-or-filename>",
  "run_b": "<run-id-or-filename>",
  "first_diverging_line": 42,
  "a_line": "...",
  "b_line": "..."
}
```

## How submission lands

Open a PR with:

1. The full sub-tree above.
2. A note in `docs/effects-spec/bounty.md` linking the PR.
3. The committed `runs/python/_summary.json` reflecting the
   submitter's local run.
4. The orchestrator (`runner/run.py`) regenerated `RESULTS.md` and
   the drift gate passes in CI.

The reviewer verifies:

- Versions pinned, no ambient configuration.
- Normalization rules documented and minimal.
- The trace artifact actually represents the agent's causal chain
  (not a hand-rolled summary).
- The N runs reproduce on a clean checkout.

## Why this slot is open

LangSmith's local trace artifacts include UUIDs and timestamps that
vary across runs by design. The hypothesis going into the bounty
window is that the byte-identical-after-normalization rate is well
below 1.0. A submission that proves the opposite — with its
normalization rules documented honestly — replaces this stub and the
published headline updates.
