# 05 — Grounding and provenance

The claim: *any value typed `Grounded<T>` traces back to at least one retrieval source, and the chain of transformations is inspectable at runtime.*

Ground­ing is the property that makes Corvid's LLM outputs auditable. Other systems let retrieval-augmented pipelines produce strings that may or may not reference the documents they cite; Corvid's compiler proves at build time that a `Grounded<T>` return was fed by retrieval, and the runtime carries the provenance chain alongside the value.

## 1. The `Grounded<T>` type

`Grounded<T>` is a wrapper type in the type system. At compile time the checker enforces the provenance obligation; at runtime, the value carries a `ProvenanceChain` exposed via `.sources()`.

```corvid
# expect: compile
effect retrieval:
    data: grounded

tool fetch_doc(id: String) -> Grounded<String> uses retrieval

prompt summarize(doc: String) -> Grounded<String>:
    "Summarize {doc}"

agent research(id: String) -> Grounded<String>:
    doc = fetch_doc(id)
    return summarize(doc)
```

- `fetch_doc` declares `uses retrieval` where `retrieval` has `data: grounded`. This is the **source declaration**.
- `summarize` consumes a `Grounded<String>` and returns a `Grounded<String>`. The wrapper propagates.
- `research`'s body ends in a value whose provenance chain includes `fetch_doc`. The check passes.

## 2. The provenance obligation

Formal rule (restated from [03 § 5](./03-typing-rules.md)):

```
  Γ ⊢ e : Grounded<τ>
  chain(e) = [s₀, s₁, …, sₙ]
  ∃ i. sᵢ has effect with data = grounded
  ─────────────────────────────────────────
  e's provenance is valid
```

The checker builds `chain(e)` during effect analysis. A chain is a sequence of the operations that contributed to `e`'s value — the retrieval tool, the prompt transformations, the agent handoffs.

Sources of a chain:

| Construct | Contributes |
|---|---|
| Tool call with `data: grounded` | Introduces a source. Chain extends by this retrieval. |
| Prompt call | Inherits sources from its arguments. The LLM call is a *transform*, not a source. |
| Agent call | Propagates sources from any `Grounded<T>` argument. |
| Literal / computation | No sources. Chain is empty. |

If an agent declares `-> Grounded<T>` but the returned value's chain is empty, the checker emits `UngroundedReturn` with a hint pointing at the missing retrieval.

## 3. Runtime provenance chain

At runtime, every `Grounded<T>` value carries a `ProvenanceChain` that records **every** operation applied to it. The chain is typed:

```
ProvenanceChain = [Step]
Step = Retrieval { source: String, timestamp: Instant, metadata: JSON }
     | PromptTransform { prompt_name: String, model: String, tokens: usize }
     | AgentHandoff { agent_name: String }
     | Severed { reason: String }
```

`Grounded<T>::sources()` returns the list. Consumers can inspect the chain at any point to answer questions like:

- "Which document was this summary derived from?"
- "Which LLM generated this classification?"
- "Did any transformation sever the chain?"

The `Severed` step documents where a chain was deliberately cut — e.g., when a synthesis step mixed grounded and ungrounded inputs, the result cannot claim grounding. That step is the runtime's equivalent of the compile-time `UngroundedReturn` error — if you sever a chain and then claim `Grounded<T>`, the runtime raises an error.

## 4. `cites ctx strictly` — runtime citation verification

Some prompts need to do more than *be grounded*; they need to *cite* the specific grounded context in the output. The `cites ctx strictly` clause on a prompt opts in to runtime citation verification:

```corvid
# expect: skip
prompt answer(doc: String, question: String) -> Grounded<String>:
    cites doc strictly
    "Given this document: {doc}\n\nAnswer: {question}"
```

When the LLM responds, the runtime verifier walks the response against `doc` and confirms every quoted span appears verbatim in the source. If the response cites text that doesn't appear in `doc`, the verifier raises `CitationViolation` with the unverifiable span.

This is stricter than most retrieval-augmented frameworks. Others check "did the model reference the document by ID?" — Corvid checks "does every verbatim quoted span exist in the document?"

See [`crates/corvid-vm/src/interp.rs`](../../crates/corvid-vm/src/interp.rs) and the `cites_strictly_param` field on `IrPrompt` for the runtime plumbing.

## 5. When grounding matters

Use cases that require `Grounded<T>`:

- **RAG pipelines**. The return value must be derivable from retrieved context, not fabricated.
- **Legal/compliance answers**. Claims must cite source material.
- **Medical/financial advice**. Outputs must reference underlying records.
- **Factual summarization**. No hallucinated quotes.

Programs that don't need grounding use plain types — `String`, `Result<T, E>`, `Option<T>`. The type wrapper is opt-in; the constraint is mandatory when the wrapper is present.

## 6. Composition with other dimensions

Grounding interacts with other dimensions through the data-union composition rule:

- `data: grounded` composes with other data categories (`financial`, `medical`, `pii`) via Union — a chain that retrieves financial records then runs a grounded transformation has `data: grounded, financial`.
- The Grounded obligation is independent: `@data(grounded)` on an agent requires `grounded` is in the composed set; `@data(financial)` requires `financial`. Both can be declared.

## 7. Interaction with `@min_confidence`

A grounded chain's confidence is still bounded by its weakest step ([04 § 4.6](./04-builtin-dimensions.md)). A retrieval at 0.99 feeding a summarization at 0.70 yields a `Grounded<String>` with composed confidence 0.70. If the agent declares `@min_confidence(0.80)`, compilation fails even though the chain is grounded.

Grounding and confidence answer different questions:
- **Grounding:** does the output *trace back* to real context?
- **Confidence:** how *certain* are the statistical judgments along the way?

A grounded, low-confidence answer is still grounded; it's just not confident. Consumers can check both properties independently.

## 8. Implementation references

- Compile-time check: `check_grounded_returns` in [../../crates/corvid-types/src/effects.rs](../../crates/corvid-types/src/effects.rs).
- Runtime chain: `GroundedValue` in [../../crates/corvid-vm/src/value.rs](../../crates/corvid-vm/src/value.rs).
- Citation verifier: search for `cites_strictly_param` in [../../crates/corvid-vm/src/interp.rs](../../crates/corvid-vm/src/interp.rs).

## Next

[06 — Confidence-gated trust](./06-confidence-gates.md) — how `autonomous_if_confident(T)` threads a runtime trust boundary through the statically-composed trust dimension.
