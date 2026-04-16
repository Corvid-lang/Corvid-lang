# Same-session ratio tools

This directory holds the publication tooling for the memory-foundation close.

## Workflow

1. `session.py` runs the shared scenarios with the required interleaving
2. it writes one canonical raw JSONL session file
3. `aggregate.py` consumes that JSONL and emits:
   - `ratios.json`
   - `ratios.md`

## Protocol encoded here

- 3 warm-up trials per stack, discarded
- 30 measured trials per stack
- strict interleaving per scenario:
  - Corvid, Python, TypeScript, repeated by `trial_idx`
- ratios only; no absolute publication tables
