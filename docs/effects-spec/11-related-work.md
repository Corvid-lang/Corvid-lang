# 11 — Related work

Every prior effect system treats effects as flat names or monadic contexts. None carry *quantitative, heterogeneously-composing dimensions*. This section compares Corvid's dimensional system against the standard references.

## 1. Koka — row polymorphism over flat effect labels

Koka (Daan Leijen, Microsoft Research) pioneered row-polymorphic effect types. A function's type carries a row of effect labels:

```koka
fun fetch(url : string) : <net,exn> string { ... }
```

Effects are labels (`net`, `exn`, `div`, `io`, ...). Composition is union over the set of labels — a function that does `<net>` and then `<exn>` has row `<net,exn>`. Algorithms operate over the row structure: row polymorphism lets higher-order functions generalize over the effects they pass through.

**What Koka has.** Static effect tracking, row polymorphism, effect handlers, algebraic effects in the style of the Eff language.

**What Koka doesn't have.** Quantitative composition. `cost` isn't expressible — labels are either present or absent, not valued. `trust` as a lattice isn't expressible — there's no lattice-aware composition archetype. Constraints like `@budget($1.00)` aren't expressible because the row doesn't carry numeric values.

**Corvid in contrast.** Dimensions are heterogeneous (Sum / Max / Min / Union / LeastReversible); values are typed (Cost, Number, Name-lattice, Bool); constraints bind to numeric or categorical bounds. Row polymorphism is not in Corvid (yet) — dimensions are fixed by the dimension table.

## 2. Eff — algebraic effects + handlers

Eff (Andrej Bauer, Matija Pretnar) introduced algebraic effects into a practical language. Programs raise operations (`print`, `read`); handlers catch and interpret them.

**What Eff has.** Handler-based effect interpretation; continuation-capturing semantics; strong denotational foundations.

**What Eff doesn't have.** Quantitative properties composed across the call graph. Handlers are about *interpreting* effects, not *bounding* them.

**Corvid in contrast.** No handlers. Effects are *properties* to prove, not operations to handle. The two systems are complementary in concept but serve different problems: Eff separates semantics from syntax; Corvid separates quantitative guarantees from mechanics.

## 3. Frank — ability-based effects

Frank (Conor McBride, Sam Lindley, Craig McLaughlin) organizes effects as *abilities* — a program has capabilities to perform certain operations. Abilities combine via union.

**What Frank has.** Clean calculus, deep type theory, ability inference.

**What Frank doesn't have.** The same gap as Eff/Koka: abilities are categorical, not quantitative.

## 4. Haskell — monad transformers, `polysemy`, `fused-effects`

Haskell encodes effects at the type level via monad transformers (`StateT s m`, `ReaderT r m`, `ExceptT e m`, …) or with effect libraries (`polysemy`, `fused-effects`) that expose an interface similar to algebraic effects.

**What Haskell has.** Type-level proof that certain effects are or aren't in the monad stack. Strong type discipline, extensive library ecosystem.

**What Haskell doesn't have.** Quantitative composition. `cost + cost = cost` is not a naturally expressible monad law — you'd encode cost as a `WriterT` over a numeric monoid and manually prove each composition preserves the budget. The tooling doesn't automate the proof.

**Corvid in contrast.** Quantitative composition is the primary abstraction. The dimension table is table-driven; the checker walks it per dimension. No monad stack to thread through; no need to lift functions into the right transformer order.

## 5. Rust `unsafe` + `Send`/`Sync`

Rust's `unsafe` block is a single flat tag — a function either contains unsafe operations or it doesn't. `Send` and `Sync` auto traits encode thread-safety properties.

**What Rust has.** Strong separation between memory-safe and unsafe code; trait-based concurrency properties.

**What Rust doesn't have.** Quantitative composition. No `cost`. No `trust` lattice. `unsafe` doesn't compose — it's binary.

**Corvid in contrast.** Every safety concern that Rust handles via `unsafe` would, in Corvid, be its own dimension. Corvid doesn't compile to native code with the guarantees Rust provides about memory safety — but its type system generalizes Rust's binary-tag approach to a table of quantitative dimensions.

## 6. Capability-based security (CapnProto, E, Oz, various research)

Capability systems give programs typed permissions — a process has a capability to access a resource or it doesn't. Capabilities are unforgeable, composable, and can be delegated.

**What cap-sec has.** Fine-grained access control, unforgeability, delegation discipline.

**What cap-sec doesn't have.** Quantitative dimensions. A capability to "call this API" is binary.

**Corvid in contrast.** Trust, data, reversible are closest in spirit to capabilities. But Corvid adds quantitative dimensions (cost, tokens, latency) and confidence (a Min-composed statistical property) alongside the categorical ones. The result is strictly more expressive for AI workflows than a capability-only system.

## 7. Linear types

Linear types (Wadler; also Rust's ownership; Idris's QTT) ensure each value is used *exactly once*. Related to session types, which encode communication protocols.

**What linear types have.** Linear resource discipline: files closed exactly once, locks released exactly once, channels matching send/receive.

**What linear types don't have.** Quantitative effects *across* linear resources. They don't track "how much money is spent" or "how confident the output is."

**Corvid in contrast.** Linear reasoning isn't in the dimensional system. But the `Weak<T>` construct and its `weak` effect row are a separate linearity-adjacent mechanism for refresh-validity of weak references.

## 8. Session types

Session types (Honda, Vasconcelos, Kubo; Rust's `session_types` crate) describe communication protocols as types. A type like `!Int. ?String. End` says "send an int, receive a string, close."

**What session types have.** Strong protocol discipline; compile-time deadlock avoidance for typed channels.

**What session types don't have.** Quantitative non-protocol properties. Cost, confidence, data categories aren't expressible.

## 9. Summary table

| System | Categorical effects? | Quantitative effects? | Heterogeneous composition rules? | Runtime-adaptive thresholds? |
|---|---|---|---|---|
| Koka | Yes | No | No | No |
| Eff | Yes | No | No | No |
| Frank | Yes | No | No | No |
| Haskell (MTL, polysemy) | Yes | With manual effort | No | No |
| Rust `unsafe` | Binary | No | No | No |
| CapSec | Yes | No | No | No |
| Linear types | Yes (resource usage) | No | No | No |
| Session types | Yes (protocol) | No | No | No |
| **Corvid dimensional** | **Yes** | **Yes — first-class** | **Yes — five archetypes** | **Yes — `autonomous_if_confident(T)`** |

## 10. What Corvid borrows

- **Koka**'s row idea — effects on function signatures.
- **Rust**'s safety-tag idea — certain operations require annotation.
- **CapSec**'s trust lattice pattern — typed authorization levels.
- **Linear/session** types' approach to resource discipline — for the `Weak<T>` subsystem.
- **Algebraic effects** literature — as a grounding for the compositionality claim.

## 11. What's novel

- **Heterogeneous composition rules per dimension.** Sum for cost, Max for trust, Min for confidence, Union for data — all in one type system, all in one row.
- **Confidence-gated trust.** Runtime-adaptive authorization based on statistical certainty.
- **`Grounded<T>`.** Runtime provenance chains + compile-time data-flow obligation + runtime citation verification (`cites ctx strictly`).
- **Quantitative constraints proved statically.** `@budget($N)` is a theorem, not a hope.
- **Mid-stream budget termination.** Live dimensional tracking during streaming output.
- **Typed model substrate (§9).** Capability routing, jurisdiction, compliance, privacy tier as first-class dimensions.
- **Custom dimensions via config.** Users extend the effect algebra without touching compiler source.
- **Proof-carrying dimensions + law-check harness.** Algebraic laws are proptested; violating dimensions cannot ship.
- **Spec↔compiler bidirectional sync.** The spec is executable; drift fails CI.
- **Self-verifying meta-test.** The verifier is verified by running the corpus against known-broken rules.

## Next

[12 — Verification methodology](./12-verification.md) — cross-tier differential verification, adversarial LLM bypass generation, preserved-semantics fuzzing, bounty-fed regression corpus, self-verifying verification.
