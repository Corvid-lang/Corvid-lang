# Provider Routing Demo

Provider-neutral chat routing across OpenAI, Anthropic, and Ollama through one
typed Corvid prompt surface.

## Setup

From this directory:

```sh
set CORVID_TEST_MOCK_LLM=1
set CORVID_TEST_MOCK_LLM_RESPONSE=provider routing selected the expected mocked response.
cargo run -q -p corvid-cli -- build
cargo run -q -p corvid-cli -- run
```

On macOS or Linux, use `export` instead of `set`.

Expected output:

```text
"provider routing selected the expected mocked response."
```

## What It Shows

- `model` declarations give each provider route typed capability and privacy
  metadata.
- The prompt route chooses OpenAI for standard work, Ollama for private local
  work, and Anthropic for deep reasoning work.
- Mock mode, replay fixtures, and real-provider mode all share the same
  `RoutedChatTurn` surface.
- The budgeted one-command path proves static cost bounds compose over the
  routed prompt call.

## Verify

From the repository root:

```sh
set CORVID_TEST_MOCK_LLM=1
set CORVID_TEST_MOCK_LLM_RESPONSE=provider routing selected the expected mocked response.
cargo run -q -p corvid-cli -- test examples/provider_routing_demo/tests/unit.cor
cargo run -q -p corvid-cli -- test examples/provider_routing_demo/tests/integration.cor
cargo run -q -p corvid-cli -- test examples/provider_routing_demo/tests/replay_invariant.cor
cargo run -q -p corvid-cli -- eval examples/provider_routing_demo/evals/provider_routing_demo.cor
cargo run -q -p corvid-cli -- replay examples/provider_routing_demo/traces/provider_routing_demo_openai.jsonl
cargo run -q -p corvid-cli -- replay examples/provider_routing_demo/traces/provider_routing_demo_ollama.jsonl
cargo run -q -p corvid-cli -- replay examples/provider_routing_demo/traces/provider_routing_demo_anthropic.jsonl
cargo run -q -p corvid-cli -- replay examples/provider_routing_demo/seed/traces/provider_routing_demo_openai.jsonl
cargo run -q -p corvid-cli -- replay examples/provider_routing_demo/seed/traces/provider_routing_demo_ollama.jsonl
cargo run -q -p corvid-cli -- replay examples/provider_routing_demo/seed/traces/provider_routing_demo_anthropic.jsonl
```

Hardening docs live under `docs/`:

- `real-providers.md` lists the opt-in OpenAI, Anthropic, and Ollama
  environment variables.
- `security-model.md` names the app-specific trust boundary, threats,
  non-goals, and adversarial test coverage.
- `runbook.md` covers deploy, observe, rollback, and incident response.

## How To Modify

Change route policy in `src/main.cor`, then update the seed prompts, Corvid
tests, eval assertions, hardening seed data, and all provider replay fixtures
together. If a provider route changes model names, keep `corvid.toml`, source
`model` declarations, replay-invariant surfaces, and trace model fields
aligned.
