# Local Model Demo Real Providers

The local model demo defaults to deterministic mock mode. Real-provider mode is
never enabled by `cargo test`, `corvid test`, or `corvid run` on a clean clone.

## Modes

- `mock`: default offline mode. The app uses `CORVID_TEST_MOCK_LLM=1` and the
  committed answer in `seed/data/local_model_provider_modes.json`.
- `replay`: deterministic regression mode. The app replays
  `traces/local_model_demo_mock_chat.jsonl` or the mirrored fixture under
  `seed/traces/`.
- `real`: live local-provider mode. The app may call Ollama only when every
  required environment variable below is present.

## Required Environment

Real mode requires all of:

```text
CORVID_RUN_REAL=1
OLLAMA_HOST=http://localhost:11434
CORVID_MODEL=ollama:llama3.2
```

The Ollama process must already be running and the selected model must be
available locally. The demo does not pull models automatically.

## Minimum Provider Version

- Ollama 0.1.45 or newer.
- A model that accepts single-turn text prompts through the Ollama HTTP API.

## Provider Contract

The Corvid app consumes the same `LocalChatTurn` shape in every mode:

```json
{
  "provider": "ollama",
  "model_id": "ollama:llama3.2",
  "question": "What does Corvid demonstrate with local models?",
  "answer": "provider-neutral local inference with deterministic replay."
}
```

Mock, replay, and real mode must keep that surface byte-compatible for the
seed question. Provider-specific usage metadata, including token counts,
latency, and local privacy classification, stays in trace host events and does
not alter the app-facing `LocalChatTurn` fields.

## CI Policy

CI runs mock and replay only. A future live-provider workflow must require
`CORVID_RUN_REAL=1`, start Ollama explicitly, record redacted traces, and avoid
uploading raw prompts or model responses unless they are deterministic demo
fixtures reviewed for disclosure risk.
