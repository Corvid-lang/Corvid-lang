# RAG QA Bot Benchmark Notes

This demo is performance-relevant at the retrieval and grounding boundary. The
committed fixture keeps CI deterministic while proving that retrieval,
grounding, prompt answering, and replay substitution compose over one typed
program.

Local smoke measurements should focus on:

- `corvid run` with `CORVID_TEST_MOCK_TOOLS` and `CORVID_TEST_MOCK_LLM=1`
- `corvid test examples/rag_qa_bot/tests/unit.cor`
- `corvid eval examples/rag_qa_bot/evals/rag_qa_bot.cor`
- `corvid replay examples/rag_qa_bot/traces/rag_qa_bot_refund_policy.jsonl`

Real embedder latency depends on provider, document count, chunk size, and
whether embeddings are already cached. Keep provider latency and quality notes
separate from the committed mock fixture so CI remains deterministic.
