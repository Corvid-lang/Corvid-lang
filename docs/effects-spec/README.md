# Corvid Effect System Specification

This directory is the normative specification of Corvid's dimensional effect system — the AI-native safety type system that has no precedent in any other language.

**Status.** Living document. Phase 20g scope. Every code example in this spec is a runnable Corvid program that is compiled and executed during spec publication; if an example breaks, the spec fails CI.

## Table of contents

| Section | Content |
|---|---|
| [00 — Overview and motivation](./00-overview.md) | What the effect system is, why Corvid has one, how it differs from prior art |
| [01 — Dimensional syntax](./01-dimensional-syntax.md) | `effect` declarations, `uses` clauses, `@constraint(...)` annotations, `DimensionValue` variants |
| [02 — Composition algebra](./02-composition-algebra.md) | Per-dimension rules (Sum, Max, Min, Union, LeastReversible), cross-dimension independence |
| [03 — Typing rules](./03-typing-rules.md) | Inference-rule notation, side conditions, soundness statement |
| [04 — Built-in dimensions](./04-builtin-dimensions.md) | Cost, trust, reversible, data, latency, confidence — with worked examples |
| [05 — Grounding and provenance](./05-grounding.md) | `Grounded<T>`, data-flow verification, `cites ctx strictly` |
| [06 — Confidence-gated trust](./06-confidence-gates.md) | `autonomous_if_confident(T)`, dynamic authorization, `@min_confidence` |
| [07 — Cost analysis and budgets](./07-cost-budgets.md) | Multi-dimensional `@budget`, worst-case path analysis, cost tree |
| [08 — Streaming effects](./08-streaming.md) | `Stream<T>`, mid-stream termination, progressive structured types |
| [09 — Typed model substrate](./09-model-substrate.md) | `model` declarations, capability-based routing, content-aware dispatch |
| [10 — FFI, generics, async interactions](./10-interactions.md) | How the effect system composes across language boundaries |
| [11 — Related work](./11-related-work.md) | Koka row polymorphism, Eff handlers, Frank abilities, Haskell monad transformers, Rust `unsafe`, capability-based security, linear types, session types |
| [12 — Verification methodology](./12-verification.md) | Cross-tier differential verification, adversarial generation, preserved-semantics fuzzing, bounty-fed regression corpus |
| [counterexamples/](./counterexamples/) | Every historical bypass attempt as a permanent regression test |

## How to read this spec

1. **If you want the 5-minute pitch**: read [00-overview.md](./00-overview.md).
2. **If you want the language-level primer**: read 01 → 02 → 04.
3. **If you want to understand a specific invention**: jump to 05–09.
4. **If you're comparing against another language**: read 11.
5. **If you want to attack the type system**: read 12, then the counterexamples directory.

## How to verify this spec

Every numbered section contains runnable examples. To verify:

```
cargo run -p corvid-cli -- test spec
```

This compiles every code block in every `.md` file in this directory against the current Corvid toolchain. Broken examples fail CI.

## Correctness guarantees

The Corvid compiler's effect system ships with five verification techniques running on every CI build:

1. **Cross-tier differential verification.** The same program's effect profile is computed by four tiers (type checker, interpreter, native codegen, replay) and they must all agree. Any divergence fails the build.
2. **Adversarial LLM-driven bypass generation.** An LLM generates programs designed to bypass the effect checker. The compiler must reject every one.
3. **Preserved-semantics fuzzing.** Programs are randomly rewritten in ways that should preserve the effect profile. If the profile changes, the analyzer is non-compositional.
4. **Mutation testing.** Known-correct programs are systematically mutated. Every mutation must be caught by the compiler.
5. **Regression corpus.** Every historical bypass attempt, including community-submitted bounties, is permanently tested. New releases cannot regress old catches.

Details of each in [12-verification.md](./12-verification.md).

## Contributing

If you find a program that should be rejected but compiles clean — or accepted but is rejected incorrectly — open an issue with the program. Accepted bypasses are credited to the reporter and added to [counterexamples/](./counterexamples/) as a permanent regression test.
