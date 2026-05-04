# RAG QA Bot Real Providers

Real provider mode is opt-in. CI and normal tests use deterministic mock
retrieval and mock LLM responses.

## OpenAI Embeddings

Environment:

```sh
CORVID_RUN_REAL=1
OPENAI_API_KEY=<redacted-openai-key>
CORVID_RAG_EMBEDDER=openai:text-embedding-3-small
```

The committed demo keeps embeddings in `examples/rag_qa_bot/seed/embeddings.json`
for offline tests. Real mode should rebuild embeddings from `seed/kb/` before a
live run, then record any provider trace through the redaction pipeline before
promotion.

## Ollama Embeddings

Minimum setup:

```sh
ollama serve
ollama pull nomic-embed-text
```

Environment:

```sh
CORVID_RUN_REAL=1
CORVID_RAG_EMBEDDER=ollama:nomic-embed-text
OLLAMA_BASE_URL=http://localhost:11434
```

`OLLAMA_BASE_URL` is optional when Ollama listens on `http://localhost:11434`.
Use it when running Ollama on another host or port.

## Mock Mode

Offline tests and CI use:

```sh
CORVID_TEST_MOCK_TOOLS={"retrieve_context":["Refunds over one hundred dollars require approval before money moves.","Refunds over one hundred dollars require approval before money moves.","Refunds over one hundred dollars require approval before money moves."]}
CORVID_TEST_MOCK_LLM=1
CORVID_TEST_MOCK_LLM_RESPONSE=Refunds over one hundred dollars require approval before money moves.
```

The mock returns the same grounded retrieval and answer strings consumed by the
Corvid program; the `RagAnswer` typed surface is unchanged.
