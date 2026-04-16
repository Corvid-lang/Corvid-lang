# Memory Foundation Deferrals

Deferred work carried forward out of the memory-foundation close. Each
item below is real engineering work with a concrete rationale for why it
waits and what must exist before it becomes worth shipping.

## `17b-6` - Effect-row-directed RC -> Effect-System Expansion

### What this work would deliver

Compile-time proof that individual `retain` / `release` pairs are
elidable, driven by richer effect information in the type system.

The moat claim behind it remains valid:

> Corvid uses effect information that competing languages do not model
> to prove ownership operations are dead in ways those languages cannot
> express structurally.

### Why it defers

Corvid v0.1 still has a binary effect model:

```rust
pub enum Effect {
    Safe,
    Dangerous,
}
```

That is enough for approval boundaries. It is not enough for
effect-row-directed ownership optimization.

To make this optimization real, the effect system needs more
granularity: something in the direction of `Pure`, `MemoryOnly`, `IO`,
`Tool`, and `LLM`, or a compositional row encoding with equivalent
power.

Without that richer vocabulary:

- "the effect row proves this call is pure" has no sound premise
- cross-function RC elimination collapses into work already handled by
  the shipped ownership pipeline
- a narrow version would either be redundant or over-claimed

This therefore belongs with the broader effect-system expansion rather
than the current memory-foundation close.

### Shortcut avoided

Shipping a narrow version on top of `Safe` / `Dangerous` would either:

- duplicate work already covered by borrow inference, pair elimination,
  and drop specialization, or
- overstate what the current type system can actually prove

The foundation is stronger without that shortcut.

### What the foundation still delivers without `17b-6`

The close still carries three innovation-grade claims:

1. **Replay-deterministic, runtime-verified ownership** via `17f++`
2. **Weak refs with effect-typed invalidation** via `17g`
3. **Latency-aware prompt / LLM boundary ownership shaping** via `17b-7`

That is already a strong close.

### What must exist before this becomes active work

- richer effect vocabulary in syntax, type checking, and IR
- effect inference or annotation rules that distinguish pure and
  effectful call graphs
- explicit interaction rules with the shipped ownership passes so the
  optimization has a canonical RC stream to prune

## `17b-3` Koka drop-guided reuse (ICFP'22) -> Deferred Research

Research-grade reuse analysis. Valuable, but not required to close the
measured foundation. It deserves a focused optimization effort rather
than being packed into the close.

## `17b-4` Morphic per-call-site specialization -> Deferred Research

Cross-function specialization by alias mode. Substantial IR and codegen
work with real upside, but it belongs in a dedicated research pass once
the current measured baseline is locked.

## `17b-5` Choi escape analysis -> Deferred Research

Escape analysis and stack promotion for non-escaping heap values.
Orthogonal to the ownership pipeline and worth doing, but not a blocker
for the close.

## VM collector locality tuning -> Deferred Research

Interpreter collection is already correct and fast enough to support the
current close. Locality tuning is a performance-polish pass, not a
foundation blocker.

## How this doc stays honest

Every deferred item here includes:

- what it would do
- why it is deferred now
- what must change before it becomes viable

That keeps the reasoning reviewable instead of burying it in historical
chat or commit messages.

## Cross-references

- Memory foundation results: [memory-foundation-results.md](memory-foundation-results.md)
- AI benchmark suite spec: [ai-benchmarks.md](ai-benchmarks.md)
- Implementation slice-by-slice audit: in the appendix of
  [memory-foundation-results.md](memory-foundation-results.md)
