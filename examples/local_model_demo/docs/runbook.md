# Local Model Demo Runbook

This runbook is for operators running the local model demo as a reference app.
The default operating mode is deterministic mock mode; real Ollama mode is
explicitly opt-in.

## Deploy

1. Clone the repository and enter `examples/local_model_demo`.
2. Keep real mode disabled for first boot:

   ```text
   CORVID_RUN_REAL=0
   ```

3. Configure deterministic mock mode:

   ```text
   CORVID_TEST_MOCK_LLM=1
   CORVID_TEST_MOCK_LLM_RESPONSE=provider-neutral local inference with deterministic replay.
   CORVID_MODEL=ollama:llama3.2
   ```

4. Build and run the app:

   ```text
   cargo run -q -p corvid-cli -- build
   cargo run -q -p corvid-cli -- run
   ```

5. Verify the one-command output is the deterministic mocked answer.
6. Run the local hardening checks from the repository root:

   ```text
   cargo run -q -p corvid-cli -- test examples/local_model_demo/tests/unit.cor
   cargo run -q -p corvid-cli -- test examples/local_model_demo/tests/integration.cor
   cargo run -q -p corvid-cli -- test examples/local_model_demo/tests/replay_invariant.cor
   cargo run -q -p corvid-cli -- replay examples/local_model_demo/traces/local_model_demo_mock_chat.jsonl
   cargo run -q -p corvid-cli -- replay examples/local_model_demo/seed/traces/local_model_demo_mock_chat.jsonl
   ```

7. For real mode, configure every variable in `docs/real-providers.md` and
   start Ollama before setting `CORVID_RUN_REAL=1`.

## Observe

1. Preserve every generated trace directory under `target/trace/` for the run
   being investigated.
2. Confirm both committed replay fixtures still pass in a plain replay
   environment with `CORVID_MODEL` unset.
3. Compare the app output to `seed/data/local_model_provider_modes.json`.
4. Check that any app-facing response includes:

   ```text
   provider
   model_id
   question
   answer
   ```

5. Treat provider or model drift as an incident unless it was part of a
   reviewed fixture update.

## Rollback

1. Disable real provider mode immediately:

   ```text
   CORVID_RUN_REAL=0
   ```

2. Clear provider env that affects plain replay:

   ```text
   CORVID_MODEL=
   CORVID_TEST_MOCK_LLM=
   CORVID_TEST_MOCK_LLM_RESPONSE=
   ```

3. Re-run both committed replay fixtures from the repository root.
4. Restore the last known-good commit that passed `demo-verify.yml`.
5. Re-run the replay invariant before re-enabling any real provider mode.

## Incident Response

1. Stop live model calls by setting `CORVID_RUN_REAL=0` and stopping Ollama if
   the incident involves local provider behavior.
2. Preserve traces, seed files, shell environment names, and commit SHA. Do not
   preserve raw prompts or model responses outside the redacted incident
   bundle.
3. Run the adversarial guarantee-id harness:

   ```text
   cargo test -p corvid-cli --test demo_project_defaults local_model_demo_adversarial_cases_carry_registered_guarantee_ids -- --nocapture
   ```

4. If the harness fails, block deployment until the replay-determinism
   rejection path is restored.
5. If the harness passes but the live model behaved unexpectedly, handle it as
   a provider/model incident. Corvid enforces the deterministic-claim boundary;
   model alignment and response quality are outside this demo's trust boundary.
6. Add a minimal redacted fixture reproducing the issue before shipping the
   fix.

## Release Checklist

- `cargo check --workspace` passes.
- `cargo test -p corvid-cli --test demo_project_defaults` passes.
- `cargo run -q -p corvid-cli -- verify --corpus tests/corpus` returns the
  established platform baseline.
- The credential-pattern scan over `examples/local_model_demo` returns no
  matches.
- `CORVID_RUN_REAL=1` is absent from default CI.
- Plain replay is run with provider env cleared.
