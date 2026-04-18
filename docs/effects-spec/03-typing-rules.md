# 03 — Typing rules

Formal inference rules for Corvid's dimensional effect system. This section tells an implementer exactly how the checker plumbs effect rows through a program and proves each `@constraint(...)` against the composed profile.

Every rule here is backed by production Rust code. Cross-references to [../../crates/corvid-types/src/effects.rs](../../crates/corvid-types/src/effects.rs) point at the specific helper or match arm that executes the rule.

## 1. Notation

We use standard judgment form. Read `Γ ⊢ e : τ ! ρ` as *"in environment Γ, expression `e` has type `τ` and effect row `ρ`."*

| Symbol | Meaning |
|---|---|
| `Γ` | Typing environment. Maps names → type + effect row. |
| `e`, `s` | Expression, statement. |
| `τ` | Base type (`Int`, `String`, `Grounded<T>`, user-declared). |
| `ρ` | Effect row — a finite map `dim_name → DimensionValue`. |
| `ε` | Empty effect row (identity under every archetype). |
| `ρ₁ ⊕ ρ₂` | Compose two rows per [02 — composition algebra](./02-composition-algebra.md). |
| `ρ ⊨ C` | Row `ρ` satisfies constraint `C`. |
| `⊢ᴅ` | Declaration judgment — a top-level `effect`/`tool`/`prompt`/`agent`. |

Effect rows compose per-dimension; `⊕` implicitly dispatches each dimension to its declared archetype's combinator (Sum, Max, Min, Union, LeastReversible).

## 2. Environment construction

Before checking any expression, the elaborator walks the file's declarations to build `Γ`:

```
⊢ᴅ effect E:
    d₁: v₁
    d₂: v₂
    ─────────────────────────
    Γ ⊢ E ↦ row{d₁: v₁, d₂: v₂}
```

Effect declarations don't add values to Γ; they register rows by name so `uses` clauses can reference them.

```
⊢ᴅ tool f(x: τ₁) -> τ₂ uses E
    ────────────────────────────────
    Γ ⊢ f : (τ₁) -> τ₂ ! row(E)
```

Prompts elaborate identically to tools. An agent declaration type-checks its body, composes the body's row, and registers the agent under that row:

```
⊢ᴅ agent g(x: τ₁) -> τ₂: body
    Γ, x: τ₁ ⊢ body : τ₂ ! ρ_body
    ────────────────────────────────
    Γ ⊢ g : (τ₁) -> τ₂ ! ρ_body
```

The body's row propagates to the agent's signature. Recursive agents converge via least-fixed-point (in practice a single pass suffices for non-recursive programs; recursion uses the declared row).

## 3. Expression rules

### 3.1 Literal

```
  ─────────────
  Γ ⊢ n : Int ! ε
```

Literals and variable references carry no effects — `ε` is the empty row.

### 3.2 Local reference

```
  x : τ ∈ Γ
  ───────────
  Γ ⊢ x : τ ! ε
```

Bindings have no effect row because effects happen at call sites, not at lookups.

### 3.3 Call

```
  Γ ⊢ f : (τ₁, …, τₙ) -> τ ! ρ_f
  Γ ⊢ aᵢ : τᵢ ! ρᵢ   for each i
  ─────────────────────────────────────────────
  Γ ⊢ f(a₁, …, aₙ) : τ ! ρ_f ⊕ ρ₁ ⊕ … ⊕ ρₙ
```

A call composes the callee's row with every argument's row. This is where dimensional composition actually happens — `cost` sums, `trust` maxes, `data` unions, etc., all in one `⊕` step.

Executed by `analyze_effects` in [../../crates/corvid-types/src/effects.rs](../../crates/corvid-types/src/effects.rs); the compositor lives in `compose_dimension`.

### 3.4 Let-binding

```
  Γ ⊢ e : τ ! ρ
  Γ, x: τ ⊢ body : τ' ! ρ'
  ─────────────────────────────────
  Γ ⊢ (let x = e in body) : τ' ! ρ ⊕ ρ'
```

A let binding composes the initializer's row with the body's row. The binding itself adds no effects (lookup is pure).

### 3.5 If

```
  Γ ⊢ cond : Bool ! ρ_c
  Γ ⊢ then : τ ! ρ_t
  Γ ⊢ else : τ ! ρ_e
  ─────────────────────────────────────────────
  Γ ⊢ (if cond: then else else) : τ ! ρ_c ⊕ (ρ_t ⊕ ρ_e)
```

Branches compose pessimistically: the `if` carries the effects of whichever branch executes, and the checker must treat *both* as possible. This is sound because no static analysis knows which branch runs.

Note the **branch-merge uses `⊕`, not a disjunctive "or"**. `cost` of `if cond: $0.01 else: $0.02` is `$0.02` (Max) — not $0.03 (Sum) — because `if` chooses one branch, and the upper bound over the choice is what matters for the constraint. *Wait* — this is subtle. `cost` composes by Sum. But `if` should take the **worst path**, not the sum. The implementation treats `if` specially: it takes the dimension-wise *worst* over the two branches, which for Sum-dimensions means `max(ρ_t, ρ_e)` and for Max-dimensions means `max(ρ_t, ρ_e)` (same result), and for Min-dimensions means `min(ρ_t, ρ_e)` (the worst weakest-link).

See `cost_tree_of` in [effects.rs](../../crates/corvid-types/src/effects.rs) for the branch handling — the code path walks both arms and records a `CostNodeKind::Branch` that the worst-case analysis treats as an upper bound, not a sum.

### 3.6 For

```
  Γ ⊢ iter : List<τ> ! ρ_i
  Γ, x: τ ⊢ body : τ' ! ρ_b
  N = static_length(iter)     if determinable
  ──────────────────────────────────────────────
  Γ ⊢ (for x in iter: body) : Unit ! ρ_i ⊕ (N × ρ_b)
```

Where `N × ρ_b` means "compose `ρ_b` with itself `N` times." For Sum dimensions this multiplies (`cost × N`); for Max/Min it's idempotent (`max(x, x) = x`); for Union it's idempotent.

When `N` is not statically determinable, the checker emits a `CostWarningKind::UnboundedLoop` and marks the cost estimate `bounded: false`. Budget constraints don't fire in that case — the analysis is honest that it can't prove a bound — but the warning surfaces so users know why.

### 3.7 Return

```
  Γ ⊢ e : τ ! ρ
  ──────────────────────
  Γ ⊢ (return e) : τ ! ρ
```

Returns propagate the returned expression's row unchanged.

## 4. Constraint satisfaction

An `@constraint(C)` on an agent asserts that the agent's composed row satisfies `C`. Formally:

```
  Γ ⊢ agent g(...) -> τ: body ! ρ_body
  @C declared on g
  ρ_body ⊨ C
  ─────────────────────────────────────
  g type-checks
```

The satisfaction judgment `ρ ⊨ C` is defined per-constraint-form:

| Constraint | `ρ ⊨ C` iff |
|---|---|
| `@trust(level)` | `ρ.trust ≤ level` in the trust lattice |
| `@budget($N)` | `ρ.cost ≤ N` and `bounded(ρ.cost) = true` |
| `@budget($N, tokens=T)` | `ρ.cost ≤ N` and `ρ.tokens ≤ T` |
| `@min_confidence(C)` | `ρ.confidence ≥ C` |
| `@reversible` | `ρ.reversible = true` |
| `@data(allowed₁, …)` | `ρ.data ⊆ {allowed₁, …}` |

When `ρ ⊨ C` fails, the checker emits an `EffectConstraintViolation` at the agent's span with the composed value, the required bound, and (for `@budget`) the worst-case path that reaches the bound.

Executed by `dimension_satisfies` in [effects.rs](../../crates/corvid-types/src/effects.rs).

## 5. `Grounded<T>` data-flow rule

`Grounded<T>` is a type, but it also carries a *provenance obligation*: any value of type `Grounded<T>` must be traceable to at least one `data: grounded` source in the call graph. The checker enforces this as a dedicated data-flow analysis.

```
  Γ ⊢ e : Grounded<τ>
  chain(e) = [s₀, s₁, …, sₙ]
  ∃ i. sᵢ has effect with data = grounded
  ─────────────────────────────────────────
  e's provenance is valid
```

Where `chain(e)` is the set of calls that contribute to `e`'s value — the provenance chain. The checker builds it during `analyze_effects`:

- A `tool` call with `data: grounded` introduces a source.
- A `prompt` call inherits sources from its inputs (the LLM call is a transform, not a source).
- An `agent` call propagates sources from any argument that's `Grounded<T>`.
- A literal or constant has no sources → chain is empty.

When an agent declares `-> Grounded<T>` but the chain is empty, the checker emits `UngroundedReturn`. The error surfaces the agent's name and a hint pointing at the provenance chain.

Runtime-side, `Grounded<T>` carries a typed `ProvenanceChain` that `.sources()` exposes — but the compile-time check already guarantees the chain is non-empty by the time the agent returns.

See `check_grounded_returns` in [effects.rs](../../crates/corvid-types/src/effects.rs).

## 6. Approve-before-dangerous rule

Separate from dimensional composition, a dangerous tool call must be preceded by a matching `approve` in the same block:

```
  Γ ⊢ f : (τ₁, …, τₙ) -> τ ! ρ_f      ρ_f.trust = human_required
  (approve Label(a₁, …, aₙ)) ∈ preceding statements in this block
  ────────────────────────────────────────────────────────────────
  Γ ⊢ f(a₁, …, aₙ) is authorized
```

If no matching approve precedes the call, the checker emits `UnapprovedDangerousCall` with a hint showing the exact approve line to add. The label matches the tool's declared label (PascalCase of the tool name by default).

## 7. Soundness

**Theorem (soundness of dimensional composition).** *If a program P type-checks under environment Γ — in particular, every `@constraint(C)` on every agent satisfies `ρ_body ⊨ C` — then every execution of P at every tier (interpreter, native, replay) produces an observed effect profile ρ' with ρ' ⊨ C.*

The theorem splits into two claims:

1. **Compositionality.** `ρ_body` is the same value regardless of the order in which effects compose, because every composition archetype is an associative, commutative semilattice or monoid (proved by `corvid test dimensions` per [02 §3.2](./02-composition-algebra.md)).

2. **Tier agreement.** The observed row ρ' on each runtime tier equals ρ_body (statically). This is the cross-tier differential-verification invariant enforced by `corvid verify --corpus` (see [../../crates/corvid-differential-verify/](../../crates/corvid-differential-verify/) and [ROADMAP](../../ROADMAP.md) Phase 20g invention #1). Any tier-disagreement is a bug in that tier — the harness tells you which.

Together: dimensional composition is compositional, the compiler computes composed rows correctly, every tier executes the same composition at runtime, and constraints proved against the computed row hold at runtime.

## 8. Worked example

```corvid
# expect: compile
effect retrieve:
    cost: $0.05
    data: grounded
    trust: autonomous

tool fetch_doc(id: String) -> String uses retrieve

prompt summarize(doc: String) -> Grounded<String>:
    "Summarize {doc}"

agent research(id: String) -> Grounded<String>:
    doc = fetch_doc(id)
    result = summarize(doc)
    return result
```

The checker's pass:

1. **Register effect.** `Γ.effects.retrieve = row{cost: $0.05, data: grounded, trust: autonomous}`.
2. **Type `fetch_doc`.** `fetch_doc : (String) -> String ! row(retrieve)`.
3. **Type `summarize`.** Prompt body is a template, row is `ε`. Signature: `summarize : (String) -> Grounded<String> ! ε`.
4. **Type `research` body:**
   - `doc = fetch_doc(id)` → `doc : String ! row(retrieve)` (row 3.3 call).
   - `result = summarize(doc)` → `result : Grounded<String> ! row(retrieve) ⊕ ε = row(retrieve)` (row 3.3 again, argument's row is `row(retrieve)`, callee's row is `ε`).
   - `return result` → body row is `row(retrieve)`.
5. **Grounded check.** `research` declares `-> Grounded<String>`. The chain for the returned `result` traces back through `summarize(doc)` → `doc` → `fetch_doc(id)` → `fetch_doc`'s `retrieve` effect with `data: grounded`. ∃ source → valid.
6. **Agent signature.** `research : (String) -> Grounded<String> ! row(retrieve)`.
7. **No constraints declared** → constraint satisfaction is vacuously true.

Now add a constraint and see it work:

```corvid
# expect: skip
@budget($0.10)
agent research(id: String) -> Grounded<String>:
    doc = fetch_doc(id)
    result = summarize(doc)
    return result
```

Row 4 (constraint satisfaction): `ρ_body.cost = $0.05 ≤ $0.10` → agent type-checks.

Tighten the budget:

```corvid
# expect: skip
@budget($0.01)
agent research(id: String) -> Grounded<String>:
    doc = fetch_doc(id)
    result = summarize(doc)
    return result
```

`ρ_body.cost = $0.05 > $0.01` → checker emits `EffectConstraintViolation` naming `cost`, the agent, the composed value, and the path (`fetch_doc` is the single contributor).

## 9. Non-goals of this section

- **Subtyping of effect rows.** Corvid rows are flat maps keyed by dimension name; there is no row-polymorphic subtyping (cf. Koka). A declaration's row is exactly the declared row — no implicit widening.
- **Effect inference for agents.** Agent effect rows are *derived* from their bodies, not *inferred* in the Hindley-Milner sense. The dimension types are fixed by the dimension table, so there's nothing to unify.
- **Linear or affine use of effects.** A call consumes its row, but the row itself isn't consumed — calling twice composes twice. Linearity is a separate concern (see the `weak` reference system, §17 of the main language reference).

## Next

[04 — Built-in dimensions](./04-builtin-dimensions.md) — cost, trust, reversible, data, latency, confidence, plus the streaming helpers `tokens` and `latency_ms`, each with their typing rule worked out and an attack-surface review.
