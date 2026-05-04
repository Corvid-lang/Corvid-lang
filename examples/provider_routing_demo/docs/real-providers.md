# Provider Routing Demo Real Providers

The provider routing demo defaults to deterministic mock mode. Real-provider
mode is never enabled by `cargo test`, `corvid test`, or `corvid run` on a
clean clone.

## Modes

- `mock`: default offline mode. The app uses `CORVID_TEST_MOCK_LLM=1` and the
  committed provider answers in `seed/data/provider_routes.json`.
- `replay`: deterministic regression mode. The app replays the OpenAI,
  Anthropic, and Ollama fixtures under `traces/` or the mirrored fixtures under
  `seed/traces/`.
- `real`: live provider mode. The app may call hosted or local providers only
  when `CORVID_RUN_REAL=1` and the provider-specific environment is present.

## Required Environment

Real mode requires the global opt-in:

```text
CORVID_RUN_REAL=1
```

Configure each live route you want to exercise:

```text
OPENAI_API_KEY=replace-with-secret-from-operator-store
ANTHROPIC_API_KEY=replace-with-secret-from-operator-store
OLLAMA_HOST=http://localhost:11434
CORVID_MODEL=ollama:llama3.2
```

The Ollama process must already be running and the selected local model must be
available. The demo does not pull models automatically.

## Minimum Provider Versions

- OpenAI Responses-compatible API available on the configured account.
- Anthropic Messages-compatible API available on the configured account.
- Ollama 0.1.45 or newer for the local route.

## Provider Contract

The Corvid app consumes the same `RoutedChatTurn` shape in every mode:

```json
{
  "policy": "standard",
  "selected_provider": "openai",
  "selected_model": "openai_fast",
  "question": "How does Corvid choose between model providers?",
  "answer": "provider routing selected the expected mocked response."
}
```

Mock, replay, and real mode must keep that app-facing surface byte-compatible
for the committed seed questions. Provider-specific usage metadata, including
token counts, latency, and invoice reconciliation data, stays in trace host
events and does not alter the `RoutedChatTurn` fields.

## CI Policy

CI runs mock and replay only. A future live-provider workflow must require
`CORVID_RUN_REAL=1`, provide credentials through the CI secret store, record
redacted traces, and avoid uploading raw provider responses unless they are
reviewed deterministic demo fixtures.
