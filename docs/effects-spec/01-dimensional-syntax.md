# 01 — Dimensional syntax

The syntactic surface of Corvid's effect system. Every construct in this section compiles against the current toolchain. If an example in this file stops compiling, the spec fails CI.

This section has two layers. The **core syntax** is what users read and write. The **meta-syntax** is what lets users extend the effect system itself — declaring new dimensions, shipping them with proofs, and publishing them to a shared registry. No other language has the second layer at all.

## 1. The `effect` declaration

An effect is a named bundle of typed dimensions.

```corvid
effect transfer_money:
    cost: $0.001
    reversible: false
    trust: human_required
    data: financial
    latency: fast
```

### 1.1 Grammar

```
effect_decl   := "effect" IDENT ":" INDENT dimension+ DEDENT
dimension     := IDENT ":" dim_value
dim_value     := number | money | IDENT | bool | confidence_gated | backpressure_policy
money         := "$" DECIMAL                           # e.g. $0.001
confidence_gated
              := "autonomous_if_confident" "(" FLOAT ")"
backpressure_policy
              := "bounded" "(" INT ")" | "unbounded"
```

Values are typed by the dimension, not by syntactic category — `cost: $0.001` is a `Money`, `trust: human_required` is a named constant from the `trust` lattice, `confidence: 0.95` is a `Number ∈ [0, 1]`. The type of each dimension value is fixed by the dimension's declaration.

### 1.2 What a dimension value can be

Corvid ships six built-in value kinds, listed in order of the concrete type they inhabit:

| Kind | Example | Built-in dimensions that use it |
|---|---|---|
| `Bool` | `reversible: false` | reversible |
| `Name` | `trust: human_required` | trust, data, latency |
| `Cost` | `cost: $0.001` | cost |
| `Number` | `confidence: 0.95` | confidence, `@min_confidence`, `@budget` bounds |
| `ConfidenceGated` | `trust: autonomous_if_confident(0.95)` | trust (only) |
| `BackpressurePolicy` | `backpressure: bounded(1000)` | backpressure (streaming only) |

### 1.3 Design tension: why these six and not more?

A smaller set (only `Name` and `Number`) forces clean semantics and forces users to think about composition laws rather than sneaking arbitrary values in. A larger set (adding `List`, `Record`, `Duration`) opens backdoors where users define dimensions whose composition rule is undecidable. We chose six as the minimum that covers every built-in dimension and every well-studied external one (GDPR jurisdictions, HIPAA compliance tiers, token caps), and we left the set extensible via custom-dimension declaration (§4) so users can add cases when they ship the proof.

**Alternative considered.** `List<Name>` as a first-class kind with `Union` composition. Rejected: `data: [financial, medical]` reads worse than `data: financial, medical` and the latter is just a shorthand for a `Set<Name>`. The list shape is redundant.

**Alternative considered.** `Duration` as a separate kind. Rejected: latency is a `Name` lattice (`fast`, `normal`, `slow`) in the type system because *wall-clock durations compose chaotically*. Exact milliseconds go in `latency_ms` when a user needs them, composed by `Sum`, typed as `Number`.

## 2. The `uses` clause

A tool, prompt, or agent declares the effects it consumes.

```corvid
# expect: skip
@tool
transfer(account: String, amount: Money) -> Result<Receipt, Error>
    uses transfer_money

prompt summarize(text: String) -> String
    uses llm_call
```

### 2.1 Grammar

```
uses_clause   := "uses" effect_name ("," effect_name)*
```

The compiler builds an **effect row** for the declaration — the set of effects it consumes. Rows compose through calls. The full algebra is in [02-composition-algebra.md](./02-composition-algebra.md).

### 2.2 Why `uses`, not `uses_effects` or `!` sigils

We considered three alternatives:

1. **`uses`** (chosen). Natural English. Parses in one token. Reads cleanly on tools and prompts.
2. **Koka-style `!` sigil** (`fn transfer() : () ! transfer_money`). Rejected: the sigil becomes a stopword that readers skip over. The effects are the point — they need a word, not a symbol.
3. **Attribute form `@effects(transfer_money)`**. Rejected: attributes are for compiler directives, not type-level declarations. Effects are part of the signature.

## 3. Constraint annotations

A constraint is an assertion the compiler proves against the composed effect profile.

```corvid
# expect: skip
@trust(autonomous)
@budget($0.50)
@min_confidence(0.90)
@reversible
agent fast_lookup(query: String) -> String:
    ...
```

### 3.1 Grammar

```
constraint    := "@" IDENT ("(" constraint_arg* ")")?
constraint_arg := dim_value | FLOAT | money
```

### 3.2 The six constraint forms

| Constraint | Dimension proved | Composition against agent body |
|---|---|---|
| `@trust(level)` | trust ≤ level | Max of body must not exceed declared level |
| `@budget($N)` | cost ≤ N | Sum of body (worst path) must not exceed N |
| `@budget($N, tokens=T)` | cost ≤ N, tokens ≤ T | Multi-dimensional |
| `@min_confidence(C)` | confidence ≥ C | Min of body must not fall below C |
| `@reversible` | reversible = true | Every effect in body must be reversible |
| `@data(allowed, …)` | data ⊆ allowed | Union of body must be a subset |

### 3.3 Why annotations at the agent, not the call site

Alternative: call-site `approve(@trust(autonomous))` wraps. Rejected: constraints are properties of the *function's public contract*, not the *caller's local decision*. A caller cannot relax a callee's contract. Annotations live at the definition site so every call is proven against the same assertion.

**Kept from alternative:** the `approve` keyword survives as a distinct construct for *authorizing* dangerous operations (runtime, per-call), not for *declaring* constraints (compile-time, per-function). See [06-confidence-gates.md](./06-confidence-gates.md).

## 4. Custom dimensions (`corvid.toml`)

**This is the invention no other effect system has.** Users extend the effect system without touching compiler source.

```toml
[effect-system.dimensions.freshness]
composition = "Max"
type = "timestamp"
default = "0"
semantics = """
Maximum age of data in a call chain. Sources stamp; transforms preserve.
Max composition: the staleness of a chain is the staleness of its
freshest source's distance from now.
"""

[effect-system.dimensions.fairness]
composition = "Max"
type = "number"
default = "0.0"
semantics = "Demographic parity gap. Compose as Max — chain is as unfair as worst link."
proof = "proofs/fairness_max_monoid.lean"
```

The compiler reads the TOML at build time and generates a new row in the dimension table:

- **`composition`** picks the composition rule from the five archetypes (`Sum`, `Max`, `Min`, `Union`, `LeastReversible`) — see [02-composition-algebra.md §3](./02-composition-algebra.md).
- **`type`** is one of the six value kinds.
- **`default`** is the identity element for the composition rule.
- **`semantics`** is prose the compiler emits in error messages.
- **`proof`** (optional) points at a machine-checkable proof of the composition rule's algebraic laws. `.lean` proofs are replayed with Lean and `.v` proofs are replayed with Coq by `corvid add-dimension` and `corvid test dimensions`.

### 4.1 Why this is powerful

Three properties no existing effect system has:

1. **Open-ended.** You can add `freshness`, `fairness`, `carbon`, `pii_minimization`, `regulatory_zone`, `model_drift`, or any dimension your domain cares about without forking the compiler.
2. **Soundness-preserving.** A custom dimension must declare its composition rule from the five archetypes. The compiler enforces the archetype's algebraic laws in CI.
3. **Community-shareable.** `corvid add-dimension fairness@1.0` pulls a registered dimension from the Corvid effect registry with its rule, its proofs, and its regression tests.

## 5. Proof-carrying dimensions

A custom dimension's declared composition rule must be proven to hold the archetype's algebraic laws. `corvid test dimensions` runs the built-in property-law harness for every dimension and replays any declared Lean/Coq proof. A dimension without a `proof` field still has to pass the property-law harness; a dimension with a proof must pass both.

```
>>> corvid test dimensions
Running dimension law checks for 3 custom dimensions…

  freshness (Max)
    associativity ..................... ok (10,000 cases)
    commutativity ..................... ok (10,000 cases)
    identity ($default = 0) ........... ok (10,000 cases)
    idempotence (x ⊕ x = x) ........... ok (10,000 cases)
    monotonicity (x ≤ x ⊕ y) .......... ok (10,000 cases)

  fairness (Max)
    all laws .......................... ok
    lean proof       proofs/fairness_max_monoid.lean  ok

  carbon (Sum)
    all laws .......................... ok
    counter-example found at x=NaN, y=0.0: NaN ⊕ 0 ≠ 0 ⊕ NaN
    REJECTED — `Sum` over floats violates commutativity at NaN
    Hint: restrict `type = "number[finite]"` or choose `Max`

1 dimension rejected.
```

A dimension whose claimed rule fails the laws **cannot ship**. The compiler refuses to load it. The registry refuses to host it. The spec cannot lie about its own extension mechanism because the extension mechanism verifies itself.

## 6. Spec-driven compiler extension

The dimension table is the authoritative definition. The compiler is a fixed engine over an extensible table. Adding a dimension is a config change, not a code change:

```
corvid.toml                       Corvid compiler
┌─────────────────────┐           ┌────────────────────┐
│ [dimensions]        │           │ const DIMENSIONS:  │
│   cost   = Sum      │  read at  │   [                │
│   trust  = Max      │ ────────► │     Dimension::of( │
│   data   = Union    │  build    │       "cost",      │
│   custom = Max      │   time    │        Sum),       │
│                     │           │     …              │
└─────────────────────┘           └────────────────────┘
```

**The moat isn't the built-in dimensions. The moat is that the system is open-ended.**

Implication: Corvid's dimension table could eventually host any domain's safety concerns — financial compliance (`sox_segregation`), medical record handling (`hipaa_minimum_necessary`), AI governance (`eu_ai_act_risk_tier`), ML experimentation (`train_test_contamination`) — without upstream compiler changes. The effect system becomes a domain-independent verification kernel that domains extend.

## 7. Bidirectional spec↔implementation sync

Every `effect` declaration in this spec is parsed by the actual Corvid parser. Every composition rule table is evaluated by the actual type checker. The spec examples ship as `.cor` files embedded in Markdown and the spec publication pipeline fails CI if any example compiles differently than the prose claims:

```
docs/effects-spec/
├── 01-dimensional-syntax.md       ← this file, containing `.cor` blocks
├── examples/
│   ├── 01_effect_decl.cor         ← extracted from §1.1
│   ├── 01_uses_clause.cor         ← extracted from §2.1
│   ├── 01_constraint.cor          ← extracted from §3
│   └── 01_custom_freshness.toml   ← extracted from §4
└── tests/
    └── spec_examples_compile.rs   ← walks examples/, runs `corvid check` on each
```

The spec and the compiler **cannot drift**. The spec is the source of truth because the spec is executable. Every commit either ships matching spec+compiler or fails CI.

## 8. Design-tension annotations

Every syntactic choice in this spec carries an **alternatives-considered block** showing what was rejected and why. §1.3, §2.2, §3.3 above are examples. The pattern is consistent across the spec. Readers who want to fork Corvid or propose changes see not just the chosen design but the design space — and the reasons past alternatives were rejected. When a future RFC proposes changing one of these, the spec's alternative-considered block is the first check: "was this rejection still valid?"

## 9. Historical evolution

The `effect` syntax has itself composed over releases. The spec records the progression so a reader can tell which forms are legacy:

```
v0.1   effect transfer_money:  cost: $0.001, reversible: false
         (comma-separated inline — deprecated)

v0.2   effect transfer_money:
           cost: $0.001
           reversible: false
         (indented block — current canonical form)

v0.3   effect transfer_money uses base_money_ops:
           cost: $0.001
         (effect inheritance — planned Phase 21)

v0.6   effect transfer_money of type financial_operation:
           cost: $0.001
         (effect typeclasses — research, not scheduled)
```

Spec updates tag each dimension/constraint with the release it landed in. A reader can tell instantly which forms are stable and which are research previews.

## 10. Cross-language equivalence counter-proofs

Every effect syntax example pairs with an appendix sketch of how other languages would express the same property — and a proof of what they miss. Worked example for §1:

### 10.1 `effect transfer_money` in Python

```python
# Attempt 1: docstring convention.
def transfer(account: str, amount: float) -> Receipt:
    """Cost: $0.001. Trust: human_required."""
    ...
```

**What this cannot do.** Compose dimensions through the call graph. Prove at compile time that an `@trust(autonomous)` caller never transitively reaches `transfer_money`. The docstring is prose; the type system doesn't read it.

### 10.2 `effect transfer_money` in TypeScript

```typescript
type TransferMoney = {
    cost: 0.001;
    reversible: false;
    trust: "human_required";
};

function transfer(
    account: string,
    amount: number
): Receipt & { __effects: TransferMoney } { ... }
```

**What this cannot do.** Compose TransferMoney with other phantom effect types. TypeScript intersection types don't have `Sum`/`Max`/`Union` composition. You would need an elaborate phantom-type encoding (§11 covers a Haskell attempt), and even then TypeScript cannot enforce `cost ≤ $0.50` because the type system has no dependent arithmetic.

### 10.3 `effect transfer_money` in Rust

```rust
#[must_use]
fn transfer<const COST_MILLICENTS: u64>(account: &str, amount: u64) -> Receipt
where
    [(); COST_MILLICENTS as usize]: Sized,
{ ... }
```

**What this cannot do.** Rust has const generics but no const-generic composition of heterogeneous dimensions, no lattice types (`human_required`), no runtime confidence gate, no provenance chain. The `unsafe` block is a single flat tag — not six independently-composing dimensions.

**Why Corvid can.** Because dimensional effects are first-class in the core language and the compiler ships with a table-driven dimensional checker. Every other language retrofits this onto a type system designed for other purposes.

---

## Toolchain commands introduced in this section

| Command | Purpose |
|---|---|
| `corvid test dimensions` | Run algebraic-law checks on every custom dimension in `corvid.toml` |
| `corvid add-dimension <name>@<version>` | Install a dimension from the Corvid effect registry |
| `corvid effect-diff <before> <after>` | Report dimension changes + composed-value drift + constraint firing changes |

See [../../crates/corvid-cli/src/main.rs](../../crates/corvid-cli/src/main.rs) for the current surface.

## Next

[02 — Composition algebra](./02-composition-algebra.md) — the five composition archetypes, their laws, and their proof obligations.
