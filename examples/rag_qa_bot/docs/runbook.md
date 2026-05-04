# RAG QA Bot Runbook

This runbook is for operators running the RAG QA bot as a reference app. The
default operating mode is deterministic mock mode; real embedder and answer
model mode is explicitly opt-in.

## Deploy

1. Clone the repository and enter `examples/rag_qa_bot`.
2. Keep real mode disabled for first boot:

   ```text
   CORVID_RUN_REAL=0
   ```

3. Configure deterministic mock mode:

   ```text
   CORVID_TEST_MOCK_TOOLS={"retrieve_context":["Refunds over one hundred dollars require approval before money moves."]}
   CORVID_TEST_MOCK_LLM=1
   CORVID_TEST_MOCK_LLM_RESPONSE=Refunds over one hundred dollars require approval before money moves.
   ```

4. Build and run the app:

   ```text
   cargo run -q -p corvid-cli -- build
   cargo run -q -p corvid-cli -- run
   ```

5. Verify the one-command output is the deterministic grounded refund-policy
   answer.
6. Run the local hardening checks from the repository root:

   ```text
   cargo run -q -p corvid-cli -- test examples/rag_qa_bot/tests/unit.cor
   cargo run -q -p corvid-cli -- test examples/rag_qa_bot/tests/integration.cor
   cargo run -q -p corvid-cli -- test examples/rag_qa_bot/tests/replay_invariant.cor
   cargo run -q -p corvid-cli -- eval examples/rag_qa_bot/evals/rag_qa_bot.cor
   cargo run -q -p corvid-cli -- replay examples/rag_qa_bot/traces/rag_qa_bot_refund_policy.jsonl
   cargo run -q -p corvid-cli -- replay examples/rag_qa_bot/seed/traces/rag_qa_bot_refund_policy.jsonl
   ```

7. For real mode, configure every variable in `docs/real-providers.md` from an
   operator-owned secret store before setting `CORVID_RUN_REAL=1`.

## Observe

1. Preserve every generated trace directory under `target/trace/` for the run
   being investigated.
2. Confirm both committed replay fixtures still pass in a plain replay
   environment with provider env cleared.
3. Compare retrieval output to `seed/data/retrieval_chunks.json` and provider
   mode expectations to `seed/data/provider_modes.json`.
4. Check that any app-facing answer includes:

   ```text
   question
   answer
   source
   grounded
   ```

5. Treat a missing source or `grounded=false` result as an incident unless it
   was produced by an explicit negative test fixture.

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
   CORVID_TEST_MOCK_TOOLS=
   OPENAI_API_KEY=
   RAG_EMBEDDER_PROVIDER=
   CORVID_EMBEDDING_MODEL=
   OLLAMA_HOST=
   ```

3. Re-run both committed replay fixtures from the repository root.
4. Restore the last known-good commit that passed `demo-verify.yml`.
5. Re-run the replay invariant and adversarial guarantee harness before
   re-enabling any real provider mode.

## Incident Response

1. Stop live embedder and answer-model calls by setting `CORVID_RUN_REAL=0` and
   removing hosted provider credentials from the runtime environment.
2. Stop Ollama if the incident involves local provider behavior.
3. Preserve traces, seed files, shell environment names, and commit SHA. Do not
   preserve raw prompts, raw provider responses, embeddings, or provider
   credentials outside the redacted incident bundle.
4. Run the adversarial guarantee-id harness:

   ```text
   cargo test -p corvid-cli --test demo_project_defaults rag_qa_bot_adversarial_cases_carry_registered_guarantee_ids -- --nocapture
   ```

5. If the harness fails, block deployment until the grounded-provenance
   rejection path is restored.
6. If the harness passes but a live provider returned an ungrounded or
   unsupported answer, handle it as a retrieval configuration, provider, or
   knowledge-base incident. Corvid enforces the grounded result boundary;
   source-document truth and model quality are outside this demo's trust
   boundary.
7. Add a minimal redacted fixture reproducing the issue before shipping the
   fix.

## Release Checklist

- `cargo check --workspace` passes.
- `cargo test -p corvid-cli --test demo_project_defaults` passes.
- `cargo run -q -p corvid-cli -- verify --corpus tests/corpus` returns the
  established platform baseline.
- The credential-pattern scan over `examples/rag_qa_bot` returns no matches.
- `CORVID_RUN_REAL=1` is absent from default CI.
- Plain replay is run with provider env cleared.
