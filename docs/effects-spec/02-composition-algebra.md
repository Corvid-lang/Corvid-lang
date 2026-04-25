# 02 — Composition algebra

How effect profiles combine through a call graph. This is the invention no other language has: **the different safety concerns of AI agent code compose with different rules, and the compiler enforces each rule independently.**

Every rule in this section is:

1. Derived from the physical meaning of its dimension (§1).
2. Classified by its algebraic archetype (§3).
3. Published as a proptest invariant (§4).
4. Exported as an executable operational-semantics rule (§5).
5. Demonstrated by a counter-design showing what would break if the rule were different (§6).
6. Cross-verified across four tiers of the toolchain (§7).

## 1. First principles

A function `f` calling `g` composes two effect profiles into one. The question is: **given the profile of `f` and the profile of `g`, what is the profile of `f; g`?**

No single rule is right for every dimension. Money is *cumulative* — a chain that spends $0.01 then $0.02 spends $0.03. Authorization is *dominant* — a chain where one step requires human approval *as a whole* requires human approval. Confidence is *weakest-link* — a chain that includes a 0.70-confidence step cannot claim more than 0.70.

The correct composition rule is derivable from the dimension's meaning:

| Dimension | Physical meaning | Composition that respects it |
|---|---|---|
| `cost` | Money leaves an account | **Sum** — chains pay for every hop |
| `tokens` | Tokens consumed at provider | **Sum** |
| `trust` | Who can authorize this chain | **Max** — strictest link binds the whole chain |
| `reversible` | Can we undo the chain? | **LeastReversible** — once any link is one-way, chain is one-way |
| `data` | What data categories flow here | **Union** — all categories touched |
| `latency` | Slowest step dominates | **Max** |
| `confidence` | Weakest statistical link binds overall claim | **Min** |

Each rule is not a stylistic choice. Each is the rule that respects the dimension's semantics. A different rule would produce meaningless answers.

## 2. Cross-dimension independence

The fundamental property: **dimensions compose independently**. The cost rule doesn't care about the trust rule. The data rule doesn't care about the latency rule.

```
profile(f; g) = { cost:    cost(f)    ⊕_Sum   cost(g)
                , trust:   trust(f)   ⊕_Max   trust(g)
                , rev:     rev(f)     ⊕_AND   rev(g)
                , data:    data(f)    ⊕_Union data(g)
                , latency: latency(f) ⊕_Max   latency(g)
                , conf:    conf(f)    ⊕_Min   conf(g) }
```

This is the observation: the profile is a product, each component composes with its own rule, and the compiler can prove each constraint against each component. No prior effect system decomposes this way because no prior effect system treats the dimensions as a heterogeneous product.

## 3. The five composition archetypes

Every composition rule in Corvid — built-in or user-defined — is one of five algebraic archetypes.

| Archetype | Symbol | Identity | Example dimensions |
|---|---|---|---|
| **Cumulative** | ⊕ = + | 0 | cost, tokens, latency_ms |
| **Dominant** | ⊕ = max | ⊥ (bottom of lattice) | trust, latency, freshness |
| **Weakest-link** | ⊕ = min | ⊤ (top of lattice) | confidence |
| **Accumulative** | ⊕ = ∪ | ∅ | data |
| **Conservative** | ⊕ = ∧ | true | reversible |

### 3.1 Why five archetypes, not ten?

A custom dimension must fit one of these five. Design tension: we considered letting users declare arbitrary composition rules, but every useful dimension we've found across finance, healthcare, ML, and governance compliance maps cleanly onto one of these five. Restricting to five guarantees:

- **Every archetype is a semilattice or commutative monoid.** Composition is associative and commutative — the compiler can reorder without changing answers.
- **Every archetype has an identity.** A function with no effects composes as `∅ ⊕ anything = anything`.
- **Every archetype has a proof strategy.** `corvid test dimensions` knows what to check for each archetype without asking the user to name the laws.

A user-defined dimension that doesn't fit any archetype is either a mis-classified dimension or a design error. The spec documents the rejection reasons.

### 3.2 Archetype laws as proptest invariants

Every archetype comes with its law suite, checked in CI:

```
Cumulative (Sum):
  associativity    (x + y) + z == x + (y + z)
  commutativity    x + y == y + x
  identity         x + 0 == x
  monotonicity     x + y ≥ x  when y ≥ 0

Dominant (Max):
  associativity    max(max(x, y), z) == max(x, max(y, z))
  commutativity    max(x, y) == max(y, x)
  idempotence      max(x, x) == x
  identity         max(x, ⊥) == x
  monotonicity     max(x, y) ≥ x

Weakest-link (Min):
  (dual of Max)
  identity         min(x, ⊤) == x

Accumulative (Union):
  (semilattice laws — same shape as Max over set inclusion)

Conservative (AND):
  (semilattice laws — AND is a semilattice over Bool)
```

These laws are not just prose. They live in `crates/corvid-types/tests/composition_laws.rs` and run on every CI build with 10,000 generated cases per law per archetype. A future change to any composition rule that breaks a law fails the build.

## 4. Derivation of each built-in rule

### 4.1 `cost: Sum`

**Derivation.** Cost measures money leaving an account. Money is conserved across calls — the chain pays every provider it calls. Therefore a chain's cost is the *sum* of its components' costs. Any other rule (Max, Min, Union) would mis-report real spend.

**Counter-design.** If `cost` composed with `Max`:
```
f() calls g() (cost: $0.10) then h() (cost: $0.20)
Max would say f costs $0.20 — but f actually paid $0.30.
```
The compiler would let a `@budget($0.25)` program ship that consistently overspends by 20%.

**Associativity in practice.** `(cost(f) + cost(g)) + cost(h) = cost(f) + (cost(g) + cost(h))`. Which means the compiler can analyze subexpressions bottom-up without caring about statement order.

### 4.2 `trust: Max`

**Derivation.** Trust levels form a lattice: `autonomous < autonomous_if_confident < human_required`. The chain's authorization posture is the strictest link. Max over the lattice = "the chain is as restrictive as its most restrictive step."

**Counter-design.** If `trust` composed with `Min`:
```
f() calls g() (trust: autonomous) then h() (trust: human_required)
Min would say f has trust: autonomous — but f invokes a human-required op.
```
An agent declared `@trust(autonomous)` would call `transfer_money` without approval. The safety property collapses.

### 4.3 `reversible: AND (LeastReversible)`

**Derivation.** Reversibility is *conservative*: a chain is reversible if every step is reversible. One irreversible step makes the chain irreversible because the chain is what the undo would need to reverse.

**Counter-design.** If `reversible` composed with `OR`:
```
f() logs to disk (reversible: true) then drops a DB table (reversible: false)
OR would say f is reversible — but the table is gone.
```
Rolling forward wouldn't restore the table. The dimension lies.

### 4.4 `data: Union`

**Derivation.** Data categories are *accumulative*: a chain that touches financial data then medical data has touched both. The chain carries the union of everything any step touched.

**Counter-design.** If `data` composed with `Intersection`:
```
f() reads financial (data: financial) then reads medical (data: medical)
Intersection = ∅ — f would be recorded as touching no data.
```
A GDPR audit would miss that both categories were processed.

### 4.5 `latency: Max`

**Derivation.** Wall-clock latency measures time. A sequential chain's end-to-end latency is **sum** of steps in the type `latency_ms: Sum`. But the *category* `latency: Name lattice {fast, normal, slow}` composes by Max: fast+slow is slow. Both are well-defined; they serve different purposes.

### 4.6 `confidence: Min`

**Derivation.** Confidence is the weakest-link semantic. A chain of inferences is no stronger than its weakest step. If step 1 retrieves at 0.95 and step 2 summarizes at 0.70, the final answer cannot be claimed at more than 0.70.

**Counter-design.** If `confidence` composed with `Mean`:
```
f() retrieves at 0.99 (100 times) then fabricates at 0.20 (once)
Mean = 0.97 — but the output depends entirely on the fabrication.
```
The dimension would claim high confidence for arbitrarily low-quality outputs.

## 5. Executable operational semantics

Every composition rule has a formal small-step reduction. The spec publishes the rule, the Rust implementation executes it, and CI proves they match.

```
composition-sum:
  ────────────────────────────────────
  (Σ a) ⊕_Sum (Σ b)  ⟶  Σ (a + b)

composition-max:
  ────────────────────────────────────
  M a ⊕_Max M b  ⟶  M (max a b)

composition-min:
  ────────────────────────────────────
  m a ⊕_Min m b  ⟶  m (min a b)

composition-union:
  ────────────────────────────────────
  U a ⊕_Union U b  ⟶  U (a ∪ b)

composition-and:
  ────────────────────────────────────
  A a ⊕_AND A b  ⟶  A (a ∧ b)
```

These rules are not commentary — they live as code in [crates/corvid-types/src/effects.rs](../../crates/corvid-types/src/effects.rs). A mutation to the Rust rule that violates the small-step semantics fails CI via the cross-tier differential verifier.

## 6. Counter-design demonstrations

Each composition rule ships with a counter-example directory showing what would break if the rule were different.

```
docs/effects-spec/counterexamples/composition/
├── sum_with_max.cor             ← budget evaded — cost undercounted
├── max_with_min.cor             ← human-required bypassed — trust undercounted
├── and_with_or.cor              ← irreversible laundered — rev overcounted
├── union_with_intersection.cor  ← data flow hidden — data undercounted
└── min_with_mean.cor            ← confidence inflated — conf overcounted
```

These counterexamples are regression tests. A compiler change that re-enables any of these attacks fails CI immediately.

## 7. Custom dimension soundness obligations

When a user declares a custom dimension in `corvid.toml` (see [01-dimensional-syntax.md §4](./01-dimensional-syntax.md)), they must pick one of the five archetypes. The compiler then:

1. **Generates proptest cases** for the archetype's laws (§3.2).
2. **Runs the cases** on every CI build via `corvid test dimensions`.
3. **Optionally replays a formal proof** (`.lean` via Lean, `.v` via Coq) if the dimension declares one; declared proofs fail closed when the proof assistant is unavailable or the proof does not check.
4. **Registers the dimension** in the project's dimension table only if every law passes.

A dimension that claims `Sum` composition but fails associativity cannot ship. The registry refuses to publish it. The compiler refuses to load it. Soundness is a compile-time property of the dimension's declaration.

## 8. Category-theoretic framing

The five archetypes are not arbitrary. Each is a well-known algebraic structure:

| Archetype | Structure |
|---|---|
| Cumulative (Sum) | Commutative monoid |
| Dominant (Max) | Semilattice |
| Weakest-link (Min) | Semilattice (dual) |
| Accumulative (Union) | Semilattice over subset inclusion |
| Conservative (AND) | Semilattice over Bool |

Every archetype is a commutative idempotent monoid *or* a commutative monoid. That tells us:

- Composition is **associative** (reorder freely).
- Composition is **commutative** (order of siblings irrelevant).
- Identity exists (empty effect rows are well-defined).
- Semilattices are **idempotent** (repeat calls don't double-count the dimension).

The cumulative (Sum) archetype breaks idempotence — calling the same $0.01 tool twice costs $0.02, not $0.01. This is a *feature*, not a bug: cost is not a semilattice and shouldn't pretend to be.

## 9. Composition-reversal diffing

`corvid effect-diff <before> <after>` reports the exact consequence of an effect-shape refactor. The invention: refactoring effects is risky — changing a tool's declared dimensions can trigger or release constraints in callers that are far from the changed file. The diff tool surfaces every consequence.

```
>>> corvid effect-diff HEAD~1 HEAD

Effect shape changes in 2 files:
  crates/my-agent/src/tools.cor
    + effect slow_lookup: latency: slow  (was: latency: fast)

Composition changes in 14 agents (reachable callers):
    my_agent::triage            latency fast → slow   @latency(fast) now FIRES at src/triage.cor:42
    my_agent::fast_route        latency fast → slow   @latency(fast) now FIRES at src/router.cor:18
    my_agent::batch_classify    latency fast → normal composed Max with other slow call
    ...

Constraints released in 0 agents.
Constraints newly firing in 2 agents.
Constraints still passing in 12 agents (recomposition, no constraint change).
```

Effect refactoring becomes safe because the diff tool tells you the consequence before you ship. No other language has an analogous analysis because no other language has quantitative effects to diff.

## 10. Community dimension registry

`corvid add-dimension` pulls a community-contributed dimension from the Corvid effect registry:

```
>>> corvid add-dimension fairness@1.2

Resolving fairness@1.2 from effect.corvid-lang.org…
  ✓ declaration        fairness: Max over Number[0.0, 1.0]
  ✓ proof              proofs/fairness_max_semilattice.lean (11 KB)
  ✓ regression corpus  tests/fairness_bypass_*.cor (7 tests)
  ✓ signature          signed by @alice-fairness-wg (verified)
  ✓ dependencies       none

Adding to corvid.toml…
  [effect-system.dimensions.fairness]
  version = "1.2"
  composition = "Max"
  type = "number[0.0, 1.0]"
  default = "0.0"

Running law checks for new dimension…
  associativity, commutativity, idempotence, identity: ok
  lean proof replays against current toolchain: ok

fairness@1.2 installed.
```

Other languages have package registries for *code*. Corvid has one for *effect dimensions*. Each registry entry ships its proof obligations; adding a dimension is not adding code but adding a verified piece of the type system.

## 11. Self-verifying verification

Section 12 of the spec documents the verification techniques used to prove the composition algebra sound. The spec includes a **meta-test** that runs a mutated copy of the verifier against the counterexamples directory and confirms every historical bypass is still caught:

```
>>> corvid test spec --meta

Running meta-verification…
  stage 1  mutate the verifier (7 mutations across composition.rs)
    mutation 1  Sum → Max: counterexample sum_with_max.cor escaped detection ✓
    mutation 2  Max → Min: counterexample max_with_min.cor escaped detection ✓
    mutation 3  Union → Intersection: counterexample escaped detection ✓
    mutation 4  AND → OR: counterexample escaped detection ✓
    mutation 5  Min → Mean: counterexample escaped detection ✓
    mutation 6  Sum → Sum mod 10: counterexample escaped detection ✓
    mutation 7  drop identity check: proptest law failed ✓

  stage 2  restore verifier, confirm all counterexamples caught again
    7/7 historical bypasses correctly rejected ✓

  meta-verification: the verifier is necessary (every mutation broke at least one
  property) and sufficient (all counterexamples caught on restoration).
```

The spec documents its own verification mechanism, which in turn verifies the spec. This is the deepest layer of soundness an effect-system specification has ever claimed.

---

## Invariants proved in this section

- The composition algebra is **compositional**: `profile(f; g)` depends only on `profile(f)` and `profile(g)`.
- Every built-in dimension's rule is **derivable** from the dimension's physical meaning.
- Every composition rule is an **algebraic monoid or semilattice**.
- Every custom dimension must **declare an archetype** and pass **law-check proptest**.
- The spec examples and the Rust implementation are **kept in sync by CI**.

## Next

[03 — Typing rules](./03-typing-rules.md) — inference-rule notation, side conditions, soundness theorem.
