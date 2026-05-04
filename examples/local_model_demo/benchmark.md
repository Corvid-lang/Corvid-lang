# Local Model Demo Benchmark Notes

This demo is performance-relevant only at the provider boundary: it proves that
a local model call can be exercised through the same Corvid prompt surface as
cloud adapters while staying deterministic under mock and replay.

Local smoke measurements should focus on:

- `corvid run` with `CORVID_TEST_MOCK_LLM=1`
- `corvid test examples/local_model_demo/tests/unit.cor`
- `corvid eval examples/local_model_demo/evals/local_model_demo.cor`
- `corvid replay examples/local_model_demo/traces/local_model_demo_mock_chat.jsonl`

Real Ollama latency depends on the selected model, hardware, and whether the
model is already loaded. Keep benchmark notes separate from the committed mock
fixture so CI remains deterministic.
