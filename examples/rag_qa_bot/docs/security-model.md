# RAG QA Bot Security Model

This document extends the canonical Corvid security model in
[`docs/security-model.md`](../../../docs/security-model.md). It does not add
new global guarantees; it maps the RAG QA bot's app-specific threats to
existing Corvid guarantees and local tests.

## Trust Boundary

```text
operator seed knowledge base
    |
    v
question -> retrieve_context tool -> Grounded<String> source chunk
    |                                  |
    |                                  v
    +------------------------> answer_from_context prompt
                                       |
                                       v
                                  RagAnswer
```

The app-specific trusted computing base is:

- Corvid parser, resolver, typechecker, runtime, replay engine, and grounded
  provenance checker as defined in the canonical security model.
- The `retrieve_context` tool declaration and `answer_from_context` prompt in
  `src/main.cor`.
- The shared retrieval and answer surfaces used by mock, replay, OpenAI, and
  Ollama modes.
- The `RagAnswer` return shape and the mock/replay/real entrypoints in
  `src/main.cor`.
- The committed seed knowledge base, chunk metadata, provider-mode fixtures,
  and redacted replay traces.
- Operator-controlled environment variables for real mode.

## Protected Assets

- Grounded provenance: every app-facing answer must retain a source chunk that
  came from the retrieval path.
- Knowledge-base integrity: seed chunks and chunk metadata must not be confused
  with untrusted answer text.
- Replay determinism: committed traces must produce the same `RagAnswer` fields
  as mock mode for the seed question.
- Provider credentials: hosted API keys and local provider endpoints must never
  enter source, seed fixtures, traces, or CI logs.
- Local privacy boundary: clean-clone CI must not contact live embedders or
  answer models by default.

## Named Threats

| Threat | Defense | Test |
| --- | --- | --- |
| `prompt_injection_retrieved_chunk` | A retrieved chunk containing instruction-like text still cannot fabricate a grounded answer without a provenance source; the checker rejects it with `grounded.provenance_required`. | `tests/adversarial/prompt_injection_retrieved_chunk.cor` |
| `ungrounded_answer` | An answer produced without a grounded retrieval source cannot be returned as `Grounded<String>`. | `tests/adversarial/ungrounded_answer.cor` |
| `kb_tampering` | A tampered source label cannot replace the retrieved grounded source when constructing the app-facing answer. | `tests/adversarial/kb_tampering.cor` |
| `replay_forgery` | A forged replay answer string does not create grounded provenance for the typed result. | `tests/adversarial/replay_forgery.cor` |

`crates/corvid-cli/tests/demo_project_defaults.rs` typechecks each adversarial
fixture and asserts the registered `grounded.provenance_required` guarantee id.

## Replay Invariant

`tests/replay_invariant.cor` asserts that mock, replay, and real RAG entrypoints
return the same `RagAnswer` fields for the deterministic seed question. Mode
selection is host configuration, not part of the typed result surface.
Embedding vectors, token counts, provider latency, and cost metadata stay in
seed fixtures or trace host events.

## Non-Goals

- This demo does not prove that an LLM ignores prompt injection. It proves that
  ungrounded answer text cannot be returned as a grounded app result.
- This demo does not prove semantic truth beyond the committed source chunk.
- Knowledge-base authoring quality, source-document accuracy, and document
  review policy are operator responsibilities.
- This demo is single-tenant. It does not claim tenant-crossing protection.
- Real provider availability, model quality, embedding drift, rate limiting, and
  provider incident response are operator responsibilities documented in
  `runbook.md`.
