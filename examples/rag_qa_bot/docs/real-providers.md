# RAG QA Bot Real Providers

The RAG QA bot defaults to deterministic mock mode. Real-provider mode is never
enabled by `cargo test`, `corvid test`, or `corvid run` on a clean clone.

## Modes

- `mock`: default offline mode. The retrieval tool uses
  `CORVID_TEST_MOCK_TOOLS` and the LLM uses `CORVID_TEST_MOCK_LLM=1`.
- `replay`: deterministic regression mode. The app replays
  `traces/rag_qa_bot_refund_policy.jsonl` or the mirrored fixture under
  `seed/traces/`.
- `real`: live provider mode. The app may call an embedder and an answer model
  only when `CORVID_RUN_REAL=1` and the provider-specific environment is
  present.

## Required Environment

Real mode requires the global opt-in:

```text
CORVID_RUN_REAL=1
```

Configure exactly one embedding path:

```text
RAG_EMBEDDER_PROVIDER=openai
OPENAI_API_KEY=replace-with-secret-from-operator-store
```

or:

```text
RAG_EMBEDDER_PROVIDER=ollama
OLLAMA_HOST=http://localhost:11434
CORVID_EMBEDDING_MODEL=ollama:nomic-embed-text
```

Configure the answer model:

```text
CORVID_MODEL=gpt-4o-mini
OPENAI_API_KEY=replace-with-secret-from-operator-store
```

or:

```text
CORVID_MODEL=ollama:llama3.2
OLLAMA_HOST=http://localhost:11434
```

The Ollama process must already be running and local models must be available.
The demo does not pull models automatically.

## Minimum Provider Versions

- OpenAI embeddings and Responses-compatible API available on the configured
  account.
- Ollama 0.1.45 or newer for local embeddings or local answer mode.

## Provider Contract

The Corvid app consumes the same `RagAnswer` shape in every mode:

```json
{
  "question": "What must happen before a large refund moves money?",
  "answer": "Refunds over one hundred dollars require approval before money moves.",
  "source": "seed/kb/refunds.md",
  "grounded": true
}
```

Mock, replay, and real mode must keep that app-facing surface byte-compatible
for the committed seed question. Embedding vectors, token counts, latency, and
provider invoices stay in seed fixtures or trace host events and do not alter
the `RagAnswer` fields.

## CI Policy

CI runs mock and replay only. A future live-provider workflow must require
`CORVID_RUN_REAL=1`, provide credentials through the CI secret store, record
redacted traces, and avoid uploading raw provider responses unless they are
reviewed deterministic demo fixtures.
