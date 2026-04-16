# Phase 17 deferrals

Slices explicitly carried forward out of Phase 17's close. Each is a
real piece of work with a concrete rationale for when it lands and
what it needs to become viable.

## `17b-6` — Effect-row-directed RC → Phase 20

### What the slice was supposed to deliver

Compile-time proof that individual `retain` / `release` pairs are
elidable, via effect-row analysis of the type system. The moat
claim: *"Corvid uses its effect rows — a type-system feature
competing languages don't have — to prove ownership ops are dead
in ways those languages structurally can't match."*

### Why it defers

Corvid v0.1's effect classification is binary:

```rust
pub enum Effect {
    Safe,
    Dangerous,
}
```

— designed for approval-gating at `approve` boundaries, not for
ownership analysis. The granularity needed for effect-row-directed
RC is richer: something like `Pure` / `MemoryOnly` / `IO` / `Tool` /
`LLM` (or a compositional row-based encoding).

Without that granularity:

- "Effect row proves this call is pure" has no premise — the
  type system doesn't know the difference between a pure arithmetic
  agent and an IO-heavy one.
- The optimization patterns that effect-row-directed RC would
  enable (pure-call-tree RC elision, cross-function retain
  elimination via effect guarantees) collapse to patterns already
  handled by the shipped `17b-1b`/`17b-1c`/`17b-2` pipeline using
  simpler mechanisms (Lean-4-style borrow inference, pair
  elimination, per-branch drop specialization).

Delivering `17b-6` honestly requires extending the effect type
system first. That's a multi-crate change touching:

- `corvid-ast` (new effect variants + grammar)
- `corvid-syntax` (parse effect annotations)
- `corvid-resolve` (effect inference for agents without explicit
  annotations)
- `corvid-types` (effect-row unification, subtyping rules)
- `corvid-ir` (effect-row metadata on every `IrAgent`)
- `corvid-codegen-cl` (`17b-6` proper, consuming the richer effects)

That's Phase 20 scope — it pairs naturally with grounding /
evaluation types / other Phase 20+ type-system expansions.

### Shortcut avoided

Trying to ship a "narrow `17b-6`" using the binary `Safe` /
`Dangerous` distinction would produce an optimization that's
either redundant with already-shipped work (if `Safe` is treated
as sort-of-pure) or unsound (if `Dangerous` is treated as
barrier-only). Neither is defensible as the innovation moat the
slice was sold as. Phase 17 ships without it rather than ships
a small optimization wearing moat branding.

### What Phase 17 still delivers without `17b-6`

Three innovation-grade claims remain in Phase 17, each backed by
code and measurement:

1. **"Replay-deterministic, runtime-verified ownership"** — `17f++`.
   The GC trigger log + shadow-count verifier prove the ownership
   optimizer's correctness on every program run with
   `CORVID_GC_VERIFY=warn`. No other refcount language ships this.

2. **"Weak refs with effect-typed invalidation"** — `17g` (Dev B).
   `Weak<T, {tool_call, llm}>` syntax uses the effect row to
   statically refuse `upgrade()` calls that cross invalidating
   effects without a refresh. The ONE Phase 17 use of effect rows
   in ownership, sized to what the current effect system supports.

3. **"Latency-aware RC across tool/LLM boundaries"** — `17b-7`
   (Dev B, in flight). Refcount-pinning at high-latency safepoints
   compresses RC overhead into moments already dominated by
   network wait. No language without AI-native primitives can make
   this optimization because no other language has a concept of
   "LLM call boundary" in its ownership model.

One moat per phase is good discipline. Three is strong.

### Pre-phase chat outline when Phase 20 picks this up

When effect-row-directed RC becomes live work again, start with:

1. **Effect vocabulary**: what's the minimum variant set that
   enables the optimization? Propose:
   `{Pure, Mem, IO, Tool, LLM, Dangerous}` with row-level
   composition. Accept/reject/refine.

2. **Inference vs. annotation**: do users write effect rows
   explicitly on agent signatures, or does the type checker infer
   them? Probably both: explicit allowed, inferred as default.

3. **Backward compatibility**: how does the richer effect system
   interact with current `Safe`/`Dangerous`-based approval
   gating? Either migrate existing semantics onto the richer
   system or layer the new variants alongside.

4. **Concrete optimizations unlocked**: for each new variant,
   state the retain/release pattern it lets us elide and the
   measurable benchmark delta we'd expect.

5. **Interaction with shipped Phase 17 passes**: `17b-6` runs
   BEFORE or AFTER the unified `dup_drop` pass? Probably after —
   the pass produces a canonical RC op stream that `17b-6` can
   prune using the richer effects.

## `17b-3` Koka drop-guided reuse (ICFP'22) → Phase 17.5

Research-paper implementation. Deserves its own phase focus.
Requires understanding the Koka paper's specific mechanism (reuse
analysis + drop specialization + per-call-site specialization) and
mapping it onto Corvid's IR. Not a blocker for Phase 17 close.

## `17b-4` Morphic per-call-site specialization → Phase 17.5

Cross-function specialization: a function called with different
refcount-liveness contexts at different sites gets a specialized
version per context. Substantial IR + codegen work. Belongs in a
focused optimization phase, not crammed into Phase 17 close.

## `17b-5` Choi escape analysis → Phase 17.5

Stack promotion for heap values that don't escape. Requires a
proper escape analysis over the IR. Orthogonal to the ownership
pass but compounds cleanly with it.

## VM collector locality tuning → Phase 17.5

The VM-tier Bacon-Rajan collector is correct and fast enough to
pass all cycle-collection tests. Locality tuning (per-block cache,
bump-arena hints, pool-recycling across cycles) is a performance
polish pass, not a foundation blocker.

---

## How this doc stays honest

Each deferred slice here has:

- A clear statement of **what it would do**
- A clear statement of **why it defers** (not "we ran out of time" —
  concrete technical reasons)
- A clear statement of **what it needs** to become viable
- An explicit target phase

If a deferral here ever needs revisiting — because a competitor
ships the same feature, because users ask for it, because Phase 20
lands earlier than expected — the reasoning above can be
challenged directly rather than unwound from scratch.

## Cross-references

- Phase 17 close-out results: [`phase-17-results.md`](phase-17-results.md)
- AI benchmark suite spec: [`ai-benchmarks.md`](ai-benchmarks.md)
- Phase 17 slice-by-slice audit: (in `phase-17-results.md` appendix)
