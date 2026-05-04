# Provider Routing Demo Real Providers

Real provider mode is opt-in. CI and normal tests use the env-backed mock LLM
adapter.

## Environment

Set the global opt-in first:

```sh
CORVID_RUN_REAL=1
```

Then configure at least one provider route:

```sh
OPENAI_API_KEY=<redacted-openai-key>
ANTHROPIC_API_KEY=<redacted-anthropic-key>
OLLAMA_BASE_URL=http://localhost:11434
```

`OLLAMA_BASE_URL` is optional when Ollama listens on `http://localhost:11434`.
Use it when running Ollama on another host or port. If local tooling uses
`OLLAMA_HOST`, set `OLLAMA_BASE_URL` to the same URL before running Corvid.

## Models

The demo catalog declares:

- `openai_fast`: OpenAI-hosted standard route.
- `anthropic_deep`: Anthropic-hosted expert route.
- `ollama_local`: local Ollama route.

The default CI path does not require any real credential:

```sh
CORVID_TEST_MOCK_LLM=1
CORVID_TEST_MOCK_LLM_RESPONSE=provider routing selected the expected mocked response.
```
