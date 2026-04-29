# Side-by-side comparisons

Each Markdown file under this directory shows the equivalent Python /
TypeScript / Go implementation of a Corvid feature line-by-line, with a
governance-line-count delta and (where applicable) a non-model
orchestration latency comparison.

These files are referenced by the Phase-done checklists for Phases 38,
39, 40, 41, 42, and 43. Their purpose is to keep the moat claim honest:
when Corvid is faster on a dimension, the comparison shows the lines or
mechanism that produces the win; when Corvid is slower or equivalent on
a raw dimension, the comparison shows the AI-native dimension on which
Corvid still wins.

## Format

Each file follows the same skeleton:

1. **Headline.** A single-sentence claim (e.g. "Corvid saves N
   governance lines vs Celery + BullMQ + Temporal on durable agent
   jobs").
2. **Reproduce.** The runner command(s) that regenerate the numbers.
3. **Side-by-side.** Three code blocks — Corvid, the Python baseline,
   the TypeScript/Node baseline (and Go where relevant) — implementing
   the same intended behaviour.
4. **Governance line count.** A table summing the lines that exist
   solely for safety / approval / audit / provenance / replay /
   confidence in each implementation.
5. **What Corvid wins on.** A precise paragraph naming the dimension:
   line-count, compile-time rejection, replayability, audit
   completeness, time-to-answer.
6. **Honesty notes.** What the comparison does *not* claim. If Corvid
   is slower on raw orchestration latency, say so.

## Files

- [`jobs_durability.md`](./jobs_durability.md) — Phase 38 reference
  comparison: durable agent jobs vs Celery + BullMQ + Temporal.
- [`auth_approval.md`](./auth_approval.md) — Phase 39 reference
  comparison: identity / tenant / approval flow vs Auth.js + FastAPI
  dependencies + Go middleware.
- [`observability.md`](./observability.md) — Phase 40 reference
  comparison: incident time-to-answer vs OpenTelemetry +
  LangSmith / Langfuse.
- [`connectors.md`](./connectors.md) — Phase 41 reference comparison:
  connector implementation cost vs raw SDK use in Python / TypeScript.

Each file is the *minimum* publishable artifact for the corresponding
phase-done bullet. Per-app comparisons (Phase 42's
`benches/comparisons/<app>.md` requirement) live alongside these and
are added as each reference app lands.

## Honesty rules

- **No strawmen.** Baselines must use the libraries a senior dev would
  actually reach for in 2026. Submissions that compare Corvid to
  "raw FastAPI without any library" or "vanilla Express without
  middleware" are not accepted.
- **No hidden Corvid wins.** If Corvid's win depends on a feature
  the baseline could trivially adopt (e.g. zod schemas for typed
  responses), the comparison says so explicitly.
- **No model-latency hiding.** Wins on orchestration latency must
  separate model-provider time from Corvid runtime time. The
  comparison reports both.
- **Numbers are reproducible.** Every comparison has a runnable
  command (or a documented manual procedure with version-pinned
  dependencies). CI runs the runner where mechanizable.
