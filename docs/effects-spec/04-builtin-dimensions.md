# 04 — Built-in dimensions

The six dimensions every Corvid program sees without declaring, plus two streaming helpers. Each has a physical meaning, an algebraic archetype, an identity element, a constraint form, and a counter-design that shows what would break with any other rule.

| Dimension | Archetype | Value type | Identity | Constraint |
|---|---|---|---|---|
| `cost` | Sum (Cumulative) | `Cost` | `$0.00` | `@budget($N)` |
| `trust` | Max (Dominant) | `Name` lattice | `autonomous` | `@trust(level)` |
| `reversible` | LeastReversible (Conservative) | `Bool` | `true` | `@reversible` |
| `data` | Union (Accumulative) | `Name` set | `none` | `@data(c₁, …)` |
| `latency` | Max (Dominant) | `Name` lattice | `instant` | *(no direct constraint)* |
| `confidence` | Min (Weakest-link) | `Number ∈ [0, 1]` | `1.0` | `@min_confidence(C)` |
| `tokens` | Sum (Cumulative) | `Number` | `0` | `@budget(tokens=T)` |
| `latency_ms` | Sum (Cumulative) | `Number` | `0` | `@budget(latency=M)` |

Each dimension's archetype is verified against its algebraic laws on every CI run via `corvid test dimensions` — see [02 § 3.2](./02-composition-algebra.md) for the law suite and [../../crates/corvid-types/src/law_check.rs](../../crates/corvid-types/src/law_check.rs) for the implementation.

---

## 4.1 `cost` — Sum

### Physical meaning

Money that leaves an account when the chain executes. Each provider call pays. A chain that calls three prompts at $0.01, $0.02, $0.03 spends $0.06 total.

### Composition

```
cost(f; g) = cost(f) + cost(g)
```

Sum under addition. Associative, commutative, identity `$0.00`, monotonic for non-negative values (all costs are non-negative in practice).

### Constraint

```corvid
# expect: skip
@budget($0.50)
agent lookup(query: String) -> String:
    ...
```

The checker computes the **worst-case path** through the agent's body (branch `max`, loop multiplication) and fails when that path exceeds the declared bound. Unbounded loops emit a warning, not an error — the checker is honest that it can't prove a bound when the loop length isn't statically known.

### Runtime behavior

Live-tracked per-token during streaming. When a streaming prompt's cumulative cost crosses `@budget`, the runtime raises `BudgetExceeded` mid-stream rather than completing the over-budget call.

### Counter-design

If `cost` composed by `Max` instead of Sum, a chain of three $0.10 calls would report `$0.10`, letting a `@budget($0.20)` agent overspend by 50%. See [`docs/effects-spec/counterexamples/composition/sum_with_max.cor`](./counterexamples/composition/sum_with_max.cor).

---

## 4.2 `trust` — Max

### Physical meaning

Authorization posture. A chain's trust level is the strictest gate any step encounters. Levels form a total order:

```
autonomous  <  supervisor_required  <  human_required
```

### Composition

```
trust(f; g) = max(trust(f), trust(g))   in the trust lattice
```

Max over the lattice. Associative, commutative, idempotent, identity `autonomous`.

### Constraint

```corvid
# expect: skip
@trust(autonomous)
agent safe_path(query: String) -> String:
    ...
```

An agent declared `@trust(autonomous)` must compose to `autonomous`. The checker refuses to compile an agent whose body reaches any call with a stricter trust level unless the declared level is loosened.

### Confidence-gated trust

A tool may declare `trust: autonomous_if_confident(0.95)`. The compiler treats it as `autonomous` statically — the agent passes `@trust(autonomous)` — but at runtime the interpreter checks composed input confidence and activates the approval gate if confidence has dropped below 0.95. See [06 — Confidence-gated trust](./06-confidence-gates.md) for the full runtime mechanism.

### Counter-design

If `trust` composed by `Min`, an agent calling one `autonomous` tool and one `human_required` tool would report `autonomous` — bypassing the approval gate on the human-required call. See [`counterexamples/composition/max_with_min.cor`](./counterexamples/composition/max_with_min.cor).

---

## 4.3 `reversible` — LeastReversible

### Physical meaning

Whether a chain can be rolled back. Reversibility is conservative: a chain is reversible iff **every** step is reversible. One irreversible step — a charged credit card, a deleted table, a sent email — makes the entire chain irreversible because the undo would need to reverse the one-way step.

### Composition

```
reversible(f; g) = reversible(f) ∧ reversible(g)
```

Logical AND. Associative, commutative, idempotent, identity `true`.

### Constraint

```corvid
# expect: skip
@reversible
agent safe_preview(id: String) -> Plan:
    ...
```

Every effect in the body must declare `reversible: true`. A single irreversible call in the chain fails the constraint with a hint naming the offending tool.

### Counter-design

If `reversible` composed by `OR`, a chain logging to disk (`reversible: true`) that also drops a database table (`reversible: false`) would report `true` — the compiler would let a `@reversible` agent silently nuke production. See [`counterexamples/composition/and_with_or.cor`](./counterexamples/composition/and_with_or.cor).

---

## 4.4 `data` — Union

### Physical meaning

Data categories that flow through a chain. A chain reading financial records then medical records has touched **both** categories — not either alone. The Union accumulates.

Well-known categories: `none`, `public`, `pii`, `financial`, `medical`, `grounded`. Users can declare more in `corvid.toml` if a dimension fork covers the domain (e.g. `hipaa_phi`, `gdpr_special_category`).

### Composition

```
data(f; g) = data(f) ∪ data(g)   (set union)
```

Union over a `Name` set. Associative, commutative, idempotent, identity `none`.

**Correctness note.** The composition rule parses each side as a comma-separated set and merges. An earlier implementation used substring-based dedup; that version failed associativity (`"pii" ⊕ ("financial" ⊕ "pii") ≠ ("pii" ⊕ "financial") ⊕ "pii"`) and was caught by [the archetype law-check harness](../../crates/corvid-types/src/law_check.rs). See [commit `66b3075`](../../ROADMAP.md) for the fix. The regression test lives in `union_data_satisfies_semilattice_laws`.

### Constraint

```corvid
# expect: skip
@data(public, grounded)
agent summarize(doc: String) -> Grounded<String>:
    ...
```

The composed set must be a subset of the allowed set. Touching `financial` when not declared fails the constraint with a hint naming the offending call chain.

### Grounding special case

A `data: grounded` entry is the marker that powers `Grounded<T>` return-type verification. See [03 § 5](./03-typing-rules.md) and [05 — Grounding and provenance](./05-grounding.md).

### Counter-design

If `data` composed by intersection, a chain touching `financial` then `medical` would report `∅` — an audit would miss both categories. See [`counterexamples/composition/union_with_intersection.cor`](./counterexamples/composition/union_with_intersection.cor).

---

## 4.5 `latency` — Max

### Physical meaning

The perceived latency class of a chain. Category lattice:

```
instant  <  fast  <  normal  <  slow  <  streaming(...)
```

Chain-level perceived latency is the slowest step's class — users feel the slow bit, not the average.

### Composition

```
latency(f; g) = max(latency(f), latency(g))
```

Max over the latency lattice. Associative, commutative, idempotent, identity `instant`.

The streaming variant composes specially: `streaming(bounded(N)) ⊕ streaming(bounded(M)) = streaming(bounded(min(N, M)))` — the tightest buffer dominates. See [`compose_latency_dimension` in effects.rs](../../crates/corvid-types/src/effects.rs).

### Constraint

No direct `@latency(...)` constraint ships in v0.1. Users needing millisecond bounds use `@budget(latency=M)` on the `latency_ms` helper dimension instead.

### Counter-design

If `latency` composed by `Min`, a chain with one `fast` and one `slow` step would report `fast` — hiding the slow bottleneck from every downstream analysis.

---

## 4.6 `confidence` — Min

### Physical meaning

Statistical confidence of the chain's output. Weakest-link semantic: a chain of inferences is no stronger than its weakest link. If a retrieval at 0.95 feeds a summarization at 0.70, the final answer cannot be claimed at more than 0.70.

### Composition

```
confidence(f; g) = min(confidence(f), confidence(g))
```

Min over `Number ∈ [0, 1]`. Associative, commutative, idempotent, identity `1.0`.

### Constraint

```corvid
# expect: skip
@min_confidence(0.90)
agent high_confidence_path(query: String) -> String:
    ...
```

The composed minimum must not fall below the declared floor. If the chain calls any step whose `confidence` is less than 0.90, the agent fails compilation.

### Runtime interaction

Confidence also gates trust via `autonomous_if_confident(T)` — see §4.2 above and [06 — Confidence-gated trust](./06-confidence-gates.md).

### Counter-design

If `confidence` composed by Mean, a chain retrieving at 0.99 (100 times) and fabricating at 0.20 (once) would report `0.97` — because Mean hides the fabrication. The output depends entirely on the fabrication, so reporting 0.97 is a lie. See [`counterexamples/composition/min_with_mean.cor`](./counterexamples/composition/min_with_mean.cor).

---

## 4.7 Helper: `tokens` — Sum

### Physical meaning

Token count consumed at the provider. Sums across prompts in the chain. Used as a secondary bound when `@budget($...)` isn't precise enough — some providers cap tokens per context window separately from cost.

### Composition

```
tokens(f; g) = tokens(f) + tokens(g)
```

Identical archetype to `cost` but typed as plain `Number`, not `Cost`. Identity `0`.

### Constraint

```corvid
# expect: skip
@budget($1.00, tokens=50000)
agent planner(query: String) -> Plan:
    ...
```

Multi-dimensional `@budget` composes both bounds independently. Either bound firing fails the constraint.

---

## 4.8 Helper: `latency_ms` — Sum

### Physical meaning

Wall-clock milliseconds. For **sequential** latency bounds where the exact ms matters (SLAs, end-to-end user-perceived latency), `latency_ms` sums across steps. Distinct from the `latency` *category* (`fast`/`slow`) which Max-composes.

### Composition

```
latency_ms(f; g) = latency_ms(f) + latency_ms(g)
```

Identity `0`. Unlike `latency` which is idempotent, `latency_ms` is Sum — calling a 50 ms tool twice takes 100 ms in a sequential chain.

### Constraint

```corvid
# expect: skip
@budget($0.50, latency=2000)
agent interactive_response(query: String) -> String:
    ...
```

Budget bound in milliseconds, same multi-dimensional form as `tokens`.

### Note

Parallel execution isn't yet modeled. When v0.2 adds `spawn` / `join` constructs, `latency_ms` over a parallel block will use `max` (the slowest branch dominates) — see ROADMAP Phase 22.

---

## 4.9 Attack-surface review

Each built-in survives every bypass attempt we've tried. The current corpus of attempted bypasses lives in [`counterexamples/composition/`](./counterexamples/composition/). New attacks are added to [`counterexamples/`](./counterexamples/) as bounty-driven regressions per [12 — Verification methodology](./12-verification.md).

Current attack coverage:
- Composition-rule confusion (sum_with_max, max_with_min, and_with_or, union_with_intersection, min_with_mean) — caught.
- Unbounded loop cost hiding — caught by `CostWarningKind::UnboundedLoop`.
- Statically-dead-branch cost undercounting — caught by the worst-case path analysis (the checker treats `if false: expensive()` as if the branch *could* execute).
- Union substring-dedup non-associativity — **caught in development** by the law-check harness itself (commit `66b3075`); the original implementation ran in production-adjacent tests without triggering this bug until 20g invention #7 landed.

The Union bug story is the clearest argument for algebraic law-checking as a first-class verification technique. No amount of example tests caught that the dedup was broken across reorder; only the archetype law check, running 10,000 random cases specifically for associativity, surfaced the counter-example. The checker shipped; the law-check ships alongside and catches its next mistake.

## Next

[05 — Grounding and provenance](./05-grounding.md) — the `Grounded<T>` type, runtime provenance chains, and `cites ctx strictly` verification.
