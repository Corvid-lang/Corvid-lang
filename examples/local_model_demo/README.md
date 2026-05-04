# Local Model Demo

Provider-neutral local LLM execution through Corvid's shared LLM adapter surface.

## Setup

From this directory:

```sh
set CORVID_TEST_MOCK_LLM=1
set CORVID_TEST_MOCK_LLM_RESPONSE=provider-neutral local inference with deterministic replay.
set CORVID_MODEL=ollama:llama3.2
cargo run -q -p corvid-cli -- build
cargo run -q -p corvid-cli -- run
```

On macOS or Linux, use `export` instead of `set`.

Expected output:

```text
"provider-neutral local inference with deterministic replay."
```

## What It Shows

- The Corvid program calls a prompt through the shared LLM runtime surface.
- Tests use the env-backed mock adapter, so the demo runs offline and in CI.
- The project config makes plain `corvid run` use the interpreter tier, which
  is where prompt provider dispatch, tracing, and replay substitution live.
- The committed trace fixture replays the chat flow without a live Ollama
  process.

## Verify

From the repository root:

```sh
set CORVID_TEST_MOCK_LLM=1
set CORVID_TEST_MOCK_LLM_RESPONSE=provider-neutral local inference with deterministic replay.
set CORVID_MODEL=ollama:llama3.2
cargo run -q -p corvid-cli -- test examples/local_model_demo/tests/unit.cor
cargo run -q -p corvid-cli -- test examples/local_model_demo/tests/integration.cor
cargo run -q -p corvid-cli -- eval examples/local_model_demo/evals/local_model_demo.cor
cargo run -q -p corvid-cli -- replay examples/local_model_demo/traces/local_model_demo_mock_chat.jsonl
```

## How To Modify

Change the prompt body or `chat` return shape in `src/main.cor`, then update
the tests, eval assertions, seed data, and replay trace together. If the demo
switches to a new local provider, keep mock, replay, and real mode on the same
typed `LocalChatTurn` surface.
