# Provider Routing Demo Runbook

This runbook is for operators running the provider routing demo as a reference
app. The default operating mode is deterministic mock mode; real provider mode
is explicitly opt-in.

## Deploy

1. Clone the repository and enter `examples/provider_routing_demo`.
2. Keep real mode disabled for first boot:

   ```text
   CORVID_RUN_REAL=0
   ```

3. Configure deterministic mock mode:

   ```text
   CORVID_TEST_MOCK_LLM=1
   CORVID_TEST_MOCK_LLM_RESPONSE=provider routing selected the expected mocked response.
   ```

4. Build and run the app:

   ```text
   cargo run -q -p corvid-cli -- build
   cargo run -q -p corvid-cli -- run
   ```

5. Verify the one-command output is the deterministic mocked answer.
6. Run the local hardening checks from the repository root:

   ```text
   cargo run -q -p corvid-cli -- test examples/provider_routing_demo/tests/unit.cor
   cargo run -q -p corvid-cli -- test examples/provider_routing_demo/tests/integration.cor
   cargo run -q -p corvid-cli -- test examples/provider_routing_demo/tests/replay_invariant.cor
   cargo run -q -p corvid-cli -- replay examples/provider_routing_demo/traces/provider_routing_demo_openai.jsonl
   cargo run -q -p corvid-cli -- replay examples/provider_routing_demo/traces/provider_routing_demo_ollama.jsonl
   cargo run -q -p corvid-cli -- replay examples/provider_routing_demo/traces/provider_routing_demo_anthropic.jsonl
   cargo run -q -p corvid-cli -- replay examples/provider_routing_demo/seed/traces/provider_routing_demo_openai.jsonl
   cargo run -q -p corvid-cli -- replay examples/provider_routing_demo/seed/traces/provider_routing_demo_ollama.jsonl
   cargo run -q -p corvid-cli -- replay examples/provider_routing_demo/seed/traces/provider_routing_demo_anthropic.jsonl
   ```

7. For real mode, configure every variable in `docs/real-providers.md` from an
   operator-owned secret store before setting `CORVID_RUN_REAL=1`.

## Observe

1. Preserve every generated trace directory under `target/trace/` for the run
   being investigated.
2. Confirm all committed replay fixtures still pass in a plain replay
   environment with provider env cleared.
3. Compare app output and route metadata to `seed/data/provider_routes.json`.
4. Check that any app-facing routed turn includes:

   ```text
   policy
   selected_provider
   selected_model
   question
   answer
   ```

5. Treat route drift as an incident unless it was part of a reviewed fixture
   and security-model update.

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
   OPENAI_API_KEY=
   ANTHROPIC_API_KEY=
   OLLAMA_HOST=
   ```

3. Re-run all committed replay fixtures from the repository root.
4. Restore the last known-good commit that passed `demo-verify.yml`.
5. Re-run the replay invariant before re-enabling any real provider mode.

## Incident Response

1. Stop live model calls by setting `CORVID_RUN_REAL=0` and removing hosted
   provider credentials from the runtime environment.
2. Stop Ollama if the incident involves local provider behavior.
3. Preserve traces, seed files, shell environment names, and commit SHA. Do not
   preserve raw prompts, raw provider responses, or provider credentials
   outside the redacted incident bundle.
4. Run the adversarial guarantee-id harness:

   ```text
   cargo test -p corvid-cli --test demo_project_defaults provider_routing_demo_adversarial_cases_carry_registered_guarantee_ids -- --nocapture
   ```

5. If the harness fails, block deployment until the replay-determinism
   rejection path is restored.
6. If the harness passes but a live provider selected an unexpected route,
   handle it as a routing configuration or provider incident. Corvid enforces
   the typed route surface and deterministic-claim boundary; provider
   availability and response quality are outside this demo's trust boundary.
7. Add a minimal redacted fixture reproducing the issue before shipping the
   fix.

## Release Checklist

- `cargo check --workspace` passes.
- `cargo test -p corvid-cli --test demo_project_defaults` passes.
- `cargo run -q -p corvid-cli -- verify --corpus tests/corpus` returns the
  established platform baseline.
- The credential-pattern scan over `examples/provider_routing_demo` returns no
  matches.
- `CORVID_RUN_REAL=1` is absent from default CI.
- Plain replay is run with provider env cleared.
