# Provider Routing Demo Security Model

This document extends the canonical Corvid security model in
[`docs/security-model.md`](../../../docs/security-model.md). It does not add
new global guarantees; it maps the provider routing demo's app-specific
threats to existing Corvid guarantees and local tests.

## Trust Boundary

```text
operator seed prompt
    |
    v
RoutedChatTurn question -> answer_with_policy prompt -> LLM adapter
       |                       |
       |                       v
       +-------------- route policy selects provider model
                               |
                               v
                    RoutedChatTurn answer
```

The app-specific trusted computing base is:

- Corvid parser, resolver, typechecker, runtime, model router, and replay
  engine as defined in the canonical security model.
- The `model` declarations and `answer_with_policy` prompt route in
  `src/main.cor`.
- The shared LLM adapter surface used by mock, replay, hosted provider, and
  Ollama local mode.
- The `RoutedChatTurn` return shape and the mock/replay/real entrypoints in
  `src/main.cor`.
- Operator-controlled environment variables for real mode.

## Protected Assets

- Provider identity: the app-facing `selected_provider` and `selected_model`
  fields must match the declared route policy for standard, private, and deep
  questions.
- Replay determinism: prompt-dependent routed flows must not be mislabeled as
  pure deterministic computations.
- Provider credentials: hosted API keys must never enter source, seed
  fixtures, traces, or CI logs.
- Local privacy boundary: private-policy runs must route to the local Ollama
  model declaration, and clean-clone CI must not contact live providers.
- Replay fixtures: committed traces must stay deterministic and redacted.

## Named Threats

| Threat | Defense | Test |
| --- | --- | --- |
| `prompt_injection_question` | A prompt-dependent routed answer cannot be marked `@deterministic`; the checker rejects it with `replay.deterministic_pure_path`. | `tests/adversarial/prompt_injection_question.cor` |
| `provider_spoofing` | Provider choice derived from untrusted prompt input cannot be claimed as deterministic provider-routing behavior. | `tests/adversarial/provider_spoofing.cor` |
| `replay_forgery` | A prompt-dependent replay answer cannot be treated as a deterministic provider-routing receipt. | `tests/adversarial/replay_forgery.cor` |

`crates/corvid-cli/tests/demo_project_defaults.rs` typechecks each adversarial
fixture and asserts the registered `replay.deterministic_pure_path` guarantee
id.

## Replay Invariant

`tests/replay_invariant.cor` asserts that mock, replay, and real provider-route
entrypoints return the same `RoutedChatTurn` fields for the deterministic seed
questions. The same test includes provider-swap safety checks for the standard
OpenAI route, private Ollama route, and deep Anthropic route. Mode selection is
host configuration, not part of the typed result surface.

## Non-Goals

- This demo does not prove that an LLM ignores prompt injection. It proves that
  prompt-dependent behavior cannot be falsely labeled deterministic by Corvid.
- This demo does not prove provider-side safety alignment, content filtering,
  rate-limit behavior, or invoice correctness.
- This demo is single-tenant. It does not claim tenant-crossing protection.
- Real provider availability, outage handling, model availability, and provider
  incident response are operator responsibilities documented in `runbook.md`.
