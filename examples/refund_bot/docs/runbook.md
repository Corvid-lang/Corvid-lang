# Refund Bot Runbook

This runbook is for operators running the refund bot as a reference app. The
default operating mode is deterministic mock mode; real provider mode is
explicitly opt-in.

## Deploy

1. Clone the repository and enter `examples/refund_bot`.
2. Keep real mode disabled for first boot:

   ```text
   CORVID_RUN_REAL=0
   ```

3. Build and run the app:

   ```text
   cargo run -q -p corvid-cli -- build
   cargo run -q -p corvid-cli -- run
   ```

4. Verify the one-command output includes `refund_bot` and
   `approval-gated refund`.
5. Run the local hardening checks from the repository root:

   ```text
   cargo run -q -p corvid-cli -- test examples/refund_bot/tests/unit.cor
   cargo run -q -p corvid-cli -- test examples/refund_bot/tests/integration.cor
   cargo run -q -p corvid-cli -- test examples/refund_bot/tests/replay_invariant.cor
   cargo run -q -p corvid-cli -- replay examples/refund_bot/traces/refund_bot_approval_gate.jsonl
   ```

6. For real mode, configure every variable in `docs/real-providers.md` from an
   operator-owned secret store. Do not write provider tokens into files under
   `examples/refund_bot`.

## Observe

1. Preserve every generated trace directory under `target/trace/` for the run
   being investigated.
2. Confirm replay still accepts the committed trace:

   ```text
   cargo run -q -p corvid-cli -- replay examples/refund_bot/traces/refund_bot_approval_gate.jsonl
   ```

3. Compare the app output to the seed data in `seed/data/refund_requests.json`.
4. Check that any refund provider result includes:

   ```text
   receipt_id
   status
   audit_id
   guarantee_id
   amount
   ```

5. Treat a missing `audit_id` or `guarantee_id` as an incident. Those fields
   are part of the typed provider surface.

## Rollback

1. Disable real provider mode immediately:

   ```text
   CORVID_RUN_REAL=0
   ```

2. Re-run the app in mock mode and verify deterministic output.
3. Restore the last known-good commit that passed `demo-verify.yml`.
4. Re-run the replay invariant test before re-enabling any provider token.
5. If a trace fixture changed during the failed rollout, discard the raw trace
   and regenerate only after the redaction pass has been reviewed.

## Incident Response

1. Freeze money-moving operations by removing `REFUND_PROVIDER_TOKEN` from the
   runtime environment.
2. Preserve current traces, seed files, shell environment names, and commit SHA.
   Do not preserve raw provider credentials in the incident bundle.
3. Run the adversarial guarantee-id harness:

   ```text
   cargo test -p corvid-cli --test demo_project_defaults refund_bot_adversarial_cases_carry_registered_guarantee_ids -- --nocapture
   ```

4. If the harness fails, block deployment until the compiler rejection path is
   restored.
5. If the harness passes but the provider performed an unexpected refund,
   escalate to provider-side incident handling. Corvid enforces the approval
   boundary before the dangerous tool call; provider settlement behavior is
   outside this demo's trust boundary.
6. Add a minimal redacted fixture reproducing the issue before shipping the
   fix.

## Release Checklist

- `cargo check --workspace` passes.
- `cargo test -p corvid-cli --test demo_project_defaults` passes.
- `cargo run -q -p corvid-cli -- verify --corpus tests/corpus` returns the
  established platform baseline.
- The credential-pattern scan over `examples/refund_bot` returns no matches.
- `CORVID_RUN_REAL=1` is absent from default CI.
