# Phase 42 External Trial Packet

## Reviewer Task

Clone the repository and run one backend reference app locally without writing a
second backend in another language.

Recommended first app:

```sh
corvid check examples/backend/personal_executive_agent/src/main.cor
corvid eval examples/backend/personal_executive_agent/evals/hardening_eval.cor
cargo test -p corvid-cli --test reference_apps phase_42_apps_have_hardening_pack_artifacts -- --nocapture --test-threads=1
```

## What To Inspect

- Routes in `examples/backend/personal_executive_agent/src/main.cor`.
- Migrations and seed data under `examples/backend/personal_executive_agent/`.
- Mock connectors and approval surface.
- `traces/demo.lineage.jsonl`.
- `security-model.md`.
- `ops/runbook.md`.

## Required Feedback

Reviewer should answer:

- Did the app run from a clean clone?
- Which command failed first, if any?
- Which app behavior felt unclear or under-documented?
- Which production claim felt too strong?
- Would this backend shape be enough to start a real internal app?

## Signoff State

Pending real external reviewer. Do not mark 42I1 or 42I2 complete until the
feedback is linked and triaged.
