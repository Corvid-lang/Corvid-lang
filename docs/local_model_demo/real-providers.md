# Local Model Demo Real Providers

Real provider mode is opt-in. CI and normal tests use the mock adapter.

## Ollama

Minimum setup:

```sh
ollama serve
ollama pull llama3.2
```

Environment:

```sh
CORVID_RUN_REAL=1
CORVID_MODEL=ollama:llama3.2
OLLAMA_BASE_URL=http://localhost:11434
```

`OLLAMA_BASE_URL` is optional when Ollama listens on `http://localhost:11434`.
Use it when running Ollama on another host or port. If your local tooling uses
`OLLAMA_HOST`, set `OLLAMA_BASE_URL` to the same URL before running Corvid.

## Mock Mode

Offline tests and CI use:

```sh
CORVID_TEST_MOCK_LLM=1
CORVID_TEST_MOCK_LLM_RESPONSE=provider-neutral local inference with deterministic replay.
CORVID_MODEL=ollama:llama3.2
```

The mock returns the same `String` prompt result consumed by the Corvid program;
the surrounding `LocalChatTurn` typed surface is unchanged.
