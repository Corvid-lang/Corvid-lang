# Local Model Demo Security Model

This document extends the canonical Corvid security model in
[`docs/security-model.md`](../../../docs/security-model.md). It does not add
new global guarantees; it maps the local model demo's app-specific threats to
existing Corvid guarantees and local tests.

## Trust Boundary

```text
operator seed prompt
    |
    v
LocalChatTurn question -> ask_local_model prompt -> LLM adapter
                                                |
                                                v
                                      LocalChatTurn answer
```

The app-specific trusted computing base is:

- Corvid parser, resolver, typechecker, runtime, and replay engine as defined
  in the canonical security model.
- The `ask_local_model` prompt declaration in `src/main.cor`.
- The shared LLM adapter surface used by mock, replay, and real Ollama mode.
- The `LocalChatTurn` return shape and the mock/replay/real entrypoints in
  `src/main.cor`.
- Operator-controlled environment variables for real mode.

## Protected Assets

- Replay determinism: prompt-dependent flows must not be mislabeled as pure
  deterministic computations.
- Provider identity: the app-facing `provider` and `model_id` fields must stay
  stable for the deterministic seed question.
- Local privacy boundary: CI and clean-clone runs must not contact a live LLM
  provider by default.
- Replay fixtures: committed traces must stay deterministic and free of raw
  provider credentials.

## Named Threats

| Threat | Defense | Test |
| --- | --- | --- |
| `prompt_injection_question` | A prompt-dependent flow cannot be marked `@deterministic`; the checker rejects it with `replay.deterministic_pure_path`. | `tests/adversarial/prompt_injection_question.cor` |
| `provider_spoofing` | Provider choice derived from prompt input cannot be claimed as deterministic local-provider behavior. | `tests/adversarial/provider_spoofing.cor` |
| `replay_forgery` | A prompt-dependent replay answer cannot be treated as a deterministic receipt. | `tests/adversarial/replay_forgery.cor` |

`crates/corvid-cli/tests/demo_project_defaults.rs` typechecks each adversarial
fixture and asserts the registered `replay.deterministic_pure_path` guarantee
id.

## Replay Invariant

`tests/replay_invariant.cor` asserts that mock, replay, and real local-chat
entrypoints return the same `LocalChatTurn` fields for the deterministic seed
question. Mode selection is host configuration, not part of the typed result
surface.

## Non-Goals

- This demo does not prove that an LLM ignores prompt injection. It proves that
  prompt-dependent behavior cannot be falsely labeled deterministic by Corvid.
- This demo does not prove Ollama model quality, safety alignment, or
  provider-side prompt filtering.
- This demo is single-tenant. It does not claim tenant-crossing protection.
- Real provider availability, model download policy, hardware capacity, and
  provider incident response are operator responsibilities documented in
  `runbook.md`.
