# Benchmark — provenance preservation rate

For each multi-hop AI workflow under `chains/`, the runner asks: at
the END of the workflow, can a downstream consumer recover the
ORIGINAL source documents that contributed to the final answer?

Corvid's `Grounded<T>` propagates provenance through every
transformation by construction. LangChain / Vercel AI SDK return
plain strings or untyped objects after the first transformation; the
citation chain dissolves unless the developer manually threads it
through every step.

The headline number is the survival rate: **of N test chains, in
how many can the final return value's type expose the source IDs
without manual bookkeeping?**

## What counts as preservation

| Stack | Counts as preserved if... |
|---|---|
| **Corvid** | The final return type is `Grounded<T>` (or contains `Grounded<T>` in a typed field), AND the provenance chain reaches back to the original retrieval call. |
| **Python + LangChain** | The final return value is a typed object whose schema declares a sources / source_documents field at the type level (not via dict access). The runner type-checks this with `mypy --strict`. |
| **TypeScript + Vercel AI SDK** | The final return type declares `sources: string[]` or equivalent at the TypeScript type level, AND the implementation populates it without manual threading from the original retrieval. |

A field that exists "by convention" — e.g. `dict[str, object]` keys
that *might* contain sources — does NOT count. The point is whether
a downstream consumer can extract provenance *without reading the
implementation*.

## Chain format

Each chain lives under `chains/<NN>-<slug>/` and contains exactly
four files:

- `chain.toml` — metadata (id, title, hops, expected outcomes).
- `corvid.cor` — the Corvid implementation. Must use idiomatic
  `Grounded<T>` types end-to-end.
- `python.py` — the equivalent Python program with the LangChain
  abstractions a senior dev would reach for.
- `typescript.ts` — the equivalent with Vercel AI SDK + zod typed
  schemas a senior dev would reach for.

### `chain.toml` schema

```toml
id = "01-rag-summarise-aggregate"
title = "Retrieve 3 docs → summarise each → aggregate into one answer"
hops = 4
description = """
The classic multi-hop RAG pattern. After three transformations
(retrieval → per-doc summary → aggregation), the citation chain
typically does not survive in unstructured-string-based
implementations.
"""

[expected]
corvid = "preserved"
python = "lost"
typescript = "lost"

corvid_provenance_type = "Grounded<String>"
python_baseline = "LangChain RetrievalQAWithSources + LLMChain"
typescript_baseline = "Vercel AI SDK streamObject + zod"
```

## How preservation is measured

The runner's classification per chain:

- For Corvid: parse `corvid.cor`, locate the final return-type
  declaration on the outer agent, assert it is `Grounded<T>` and
  that there is a `provenance(...)` call accessible at the call
  site.
- For Python: parse `python.py` for the final function's return-type
  annotation. If the annotation is a TypedDict / Pydantic model with
  a typed `sources` / `source_documents` field, score `preserved`.
  If the annotation is `str` / `dict` / untyped, score `lost`.
- For TypeScript: same idea — parse the exported function's return
  type. If it has a `sources: string[]` field at the TS-type level,
  preserved. Otherwise lost.

The runner is static-only — no LLM calls run. The point is the
TYPE-LEVEL guarantee, not whether a particular runtime passes.

## How to add a chain

1. `mkdir chains/<NN>-<slug>/`
2. Write `chain.toml`, `corvid.cor`, `python.py`, `typescript.ts`.
3. Run `python runner/run.py --chains-dir chains --out RESULTS.md`.
4. Commit. CI's drift gate diffs the runner output against the
   committed `RESULTS.md`.

## Honesty rules

1. **Idiomatic implementations.** LangChain code uses LangChain's
   typed surfaces (`RetrievalQAWithSources`, `LLMChain`, zod schemas
   for Vercel). Naive implementations that use `dict[str, Any]`
   throughout are rejected — they would let Corvid win against an
   unfair baseline.
2. **No "winning by under-implementing."** Every chain must produce
   functionally equivalent output across stacks. If the Python
   version skips a hop, the runner logs it as a missing feature
   line, not zero provenance lines.
3. **Adversarial review.** Implementations go to
   `docs/effects-spec/bounty.md` for ≥7 days before publishing; if
   a reviewer submits a more-idiomatic Python/TS rewrite that
   preserves provenance through the chain, the rewrite replaces the
   published version and the score updates accordingly.

## Path to first headline

3 seed chains commit alongside the runner. Target is 10 chains for
the publishable headline. Until then, `RESULTS.md` reports partial
coverage explicitly.
