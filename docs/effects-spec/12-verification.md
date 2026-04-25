# 12 — Verification methodology

Five independent techniques run on every build. A regression in any one fails CI. Together they provide a stronger soundness claim than any existing language's type system has made.

## 1. Cross-tier differential verification

**What it checks.** Corvid has four execution tiers: type checker (static), interpreter (dynamic, portable), native codegen (dynamic, AOT), replay (deterministic re-execution of recorded traces). The harness runs every program through all four and reports if any tier disagrees on the composed effect profile.

**Why it matters.** If the checker says `@trust(autonomous)` is satisfied but the interpreter triggers an approval gate at runtime, *one of the tiers is lying*. The harness catches the divergence and classifies it:

- `static-overapprox` — checker was stricter than runtime. Acceptable (the checker is being conservative).
- `static-too-loose` — checker missed something runtime observed. *Real soundness bug* — fail the build.
- `tier-mismatch` — two runtime tiers disagree with each other. Likely a codegen bug.

**Implementation.** [`crates/corvid-differential-verify/`](../../crates/corvid-differential-verify/). Public API: `verify_program(path) -> DivergenceReport`, `verify_corpus(dir) -> Vec<DivergenceReport>`. CLI: `corvid verify --corpus <dir>`. Shipped in commit `d89c910` + native-tier follow-ups in commits `3b1a380`/`9616c20`/`7d63e1c`.

**Corpus.** `tests/corpus/` — 20 programs that agree across all four tiers, plus `should_fail/tier_disagree.cor` and `should_fail/native_drops_effect.cor` that prove the harness catches divergences. The deliberate-fail fixtures are the harness testing *itself*.

**Inventive angles shipped.**
- **Minimum-divergent shrinker.** `corvid verify --shrink P.cor` reduces a divergent program to its smallest reproducer.
- **Blame attribution.** On divergence, `git blame` runs on the tier's dimension-computing file and the report cites the responsible commit + author.
- **Soundness lattice classification.** Every divergence is classified as one of the three categories above so users know whether to act.

## 2. Adversarial LLM-driven bypass generation

**What it checks.** An LLM-driven generator proposes programs designed to bypass the dimensional effect checker. The test suite runs every generated program through `corvid check`. The compiler must reject every one.

**Why it matters.** If the LLM finds a program that compiles clean but the spec says should fail, either (a) the LLM found a real bypass and the checker needs fixing, or (b) the program is actually legal and the prompt was wrong. Either outcome surfaces an issue worth resolving.

**Implementation status.** `corvid test adversarial --count N --model M` now runs a deterministic seed generator plus the same accept/reject classifier used for future provider-backed attempts. The shipped taxonomy covers approval, trust, budget, provenance, reversibility, and confidence bypass families. Every generated `.cor` program goes through the full compiler frontend. A clean compile is reported as `ESCAPED` and makes the command exit non-zero.

**Issue filing.** Escaped bypasses can file GitHub issues automatically when `CORVID_ADVERSARIAL_FILE_ISSUES=1` and `GITHUB_TOKEN` are set. Without those variables the suite remains CI-safe and offline: escaped rows still fail the command, but no network side effect is attempted.

**Provider generation.** The `--model` flag is recorded in the report and prompt pack. Live LLM sampling is intentionally separate from the deterministic seed path so CI can exercise the soundness harness without API spend or nondeterminism.

## 3. Preserved-semantics fuzzing

**What it checks.** Programs get randomly rewritten in ways that *should* preserve the composed effect profile — α-conversion, let-extract/inline, commutative swap, top-level reorder, constant folding, branch-arm swap. After each rewrite, the harness compares the original's effect profile to the rewritten version's. They must match.

**Why it matters.** If profiles diverge under a rewrite that claims to preserve semantics, the effect analyzer is *non-compositional* — it depends on surface syntax rather than semantics. That's a real soundness bug.

**Implementation status.** Scaffold landed in commit `d89c910`. Real AST-level rewrites are Dev B's Phase 20g invention #4 track — slice A shipped at commit `b300fd2` (alpha-conversion + let-extract + let-inline); slice B added commutative sibling swap, top-level reorder, branch swap, and constant folding; slice C exposes the matrix as `corvid test rewrites` and reports any drift with the rewrite rule, semantic law, first changed line, profile diff, and shrunk reproducer.

**Inventive angle.** Each rewrite carries a **law reference**. When a rewrite breaks a profile, the divergence report cites the law: "α-equivalence broken at path/to/file.cor:42." Users learn the algebra by reading failures.

## 4. Bounty-fed regression corpus

**What it checks.** Every historical bypass attempt lives permanently in the codebase. New releases must reject every historical bypass. Contributors who find new bypasses get credit + a permanent entry in the corpus.

**Why it matters.** Soundness claims compound: the more attacks a verifier has survived, the more credible future claims become. Bounty-fed corpora have delivered for SAT solvers, cryptographic libraries, and fuzzers; no prior effect system has one.

**Implementation status.** [`docs/effects-spec/counterexamples/composition/`](./counterexamples/composition/) holds the seed corpus (five composition attacks). Each fixture names the bypass, bug exposed, fix/proof mechanism, and seed-corpus credit. The meta-verification harness (see §5) uses this corpus today. The public process is live in [`bounty.md`](./bounty.md), with a structured GitHub issue template at [`.github/ISSUE_TEMPLATE/effect-bypass.yml`](../../.github/ISSUE_TEMPLATE/effect-bypass.yml).

**What's live today.** The corpus directory, the meta-verification harness consuming it, the CI gate that keeps the harness passing, the submission guidelines, disclosure rules, credit format, and issue template.

## 5. Self-verifying verification (meta-verification)

**What it checks.** The counter-example corpus is only useful if each fixture actually *distinguishes* its correct composition rule from the attacker's wrong rule. The meta-verifier checks this property:

- For each counter-example, compute the composed value under the **correct** rule.
- Compute the composed value with the target dimension's rule swapped for the **attacker's** rule.
- Assert the two values differ.

**Why it matters.** If a fixture doesn't distinguish, it catches nothing — it's dead weight that looks like coverage. The meta-verifier flags degenerate fixtures so they get regenerated.

**Implementation.** `corvid-driver::meta_verify` module. Public API: `verify_counterexample_corpus(dir) -> Vec<MetaVerdict>`. CLI: `corvid test spec --meta`. Shipped in commit `e368ebb`.

**What it proves.** The verifier is both **necessary** (every attacker rule breaks at least one fixture) and **sufficient** (every fixture passes on the correct rules). This is the deepest soundness claim an effect-system specification has ever made.

## 6. Algebraic-law verification (dimension proptest harness)

**What it checks.** Every dimension's claimed archetype (Sum / Max / Min / Union / LeastReversible) must satisfy the algebraic laws the archetype claims — associativity, commutativity, identity, and (for semilattices) idempotence, monotonicity.

**Why it matters.** A dimension whose composition claims to be associative but isn't produces order-dependent results — two programs with the same calls in different order would get different composed profiles, which makes compile-time proofs about the profile meaningless.

**Implementation.** `corvid-types::law_check` module. 10,000 proptest cases per law per archetype. CLI: `corvid test dimensions`. Shipped in commit `66b3075`.

**What it caught.** The `Union` composition rule's original substring-based dedup was non-associative. `"pii" ⊕ ("financial" ⊕ "pii") ≠ ("pii" ⊕ "financial") ⊕ "pii"`. The law-check harness caught the counterexample during development; the fix (set-based dedup) shipped alongside the harness. No example-based test suite would have surfaced this bug.

## 7. Spec↔compiler bidirectional sync

**What it checks.** Every ```corvid example in [00-overview.md](./00-overview.md) through this section is parsed by the real compiler. Each block declares its expectation (`# expect: compile` / `# expect: error "pattern"` / `# expect: skip`). The harness runs each block and fails if outcome diverges from expectation.

**Why it matters.** The spec cannot lie about the compiler's behavior because the spec's examples *are* the compiler's test cases. A change to the composition algebra that breaks an example fails CI; a change to the spec that claims a different outcome fails CI.

**Implementation.** `corvid-driver::spec_check` module. CLI: `corvid test spec`. Shipped in commit `413b39e`.

## 8. Rule-to-test cross-links

| Spec rule family | Production rule implementation | Property / regression tests | Differential / corpus gate |
|---|---|---|---|
| Composition archetypes (§2, §6) | [`crates/corvid-types/src/effects/compose.rs`](../../crates/corvid-types/src/effects/compose.rs), [`crates/corvid-types/src/law_check.rs`](../../crates/corvid-types/src/law_check.rs) | `corvid test dimensions`; [`crates/corvid-driver/tests/custom_dimensions.rs`](../../crates/corvid-driver/tests/custom_dimensions.rs) | `corvid test spec --meta`; [`docs/effects-spec/counterexamples/composition/`](./counterexamples/composition/) |
| Constraint satisfaction and budgets (§3, §7) | [`crates/corvid-types/src/checker.rs`](../../crates/corvid-types/src/checker.rs), [`crates/corvid-types/src/effects/cost.rs`](../../crates/corvid-types/src/effects/cost.rs) | [`crates/corvid-types/src/tests.rs`](../../crates/corvid-types/src/tests.rs) budget and mutation suites | [`tests/corpus/`](../../tests/corpus/) through `corvid verify --corpus tests/corpus` |
| `Grounded<T>` provenance (§3, §5) | [`crates/corvid-types/src/effects/grounded.rs`](../../crates/corvid-types/src/effects/grounded.rs), [`crates/corvid-vm/src/value.rs`](../../crates/corvid-vm/src/value.rs) | [`crates/corvid-types/src/tests.rs`](../../crates/corvid-types/src/tests.rs) provenance mutations; [`crates/corvid-vm/src/tests/dispatch.rs`](../../crates/corvid-vm/src/tests/dispatch.rs) | `corvid verify --corpus tests/corpus` plus strict-citation VM/native parity tests |
| Approve-before-dangerous (§3) | [`crates/corvid-types/src/checker/stmt.rs`](../../crates/corvid-types/src/checker/stmt.rs), [`crates/corvid-types/src/checker/call.rs`](../../crates/corvid-types/src/checker/call.rs) | [`crates/corvid-types/src/tests.rs`](../../crates/corvid-types/src/tests.rs) approval mutation suite | `corvid verify --corpus tests/corpus`; trace-diff approval deltas in [`crates/corvid-cli/src/trace_diff/`](../../crates/corvid-cli/src/trace_diff/) |
| Confidence gates (§6) | [`crates/corvid-types/src/checker/prompt.rs`](../../crates/corvid-types/src/checker/prompt.rs), [`crates/corvid-vm/src/interp.rs`](../../crates/corvid-vm/src/interp.rs) | [`crates/corvid-types/src/tests.rs`](../../crates/corvid-types/src/tests.rs) confidence tests; [`crates/corvid-vm/src/tests/dispatch.rs`](../../crates/corvid-vm/src/tests/dispatch.rs) runtime gate tests | `corvid test dimensions` covers Min composition for confidence |
| Preserved-semantics rewrites (§3) | [`crates/corvid-differential-verify/src/rewrite.rs`](../../crates/corvid-differential-verify/src/rewrite.rs), [`crates/corvid-differential-verify/src/fuzz.rs`](../../crates/corvid-differential-verify/src/fuzz.rs) | [`crates/corvid-differential-verify/tests/rewrite_ast.rs`](../../crates/corvid-differential-verify/tests/rewrite_ast.rs) | `corvid test rewrites` |
| Cross-tier profile agreement (§1) | [`crates/corvid-differential-verify/src/lib.rs`](../../crates/corvid-differential-verify/src/lib.rs) | Deliberate fail fixtures [`tests/corpus/should_fail/`](../../tests/corpus/should_fail/) | `corvid verify --corpus tests/corpus` |

## 9. CI gates

Every one of the above runs on every push and every pull request. [`.github/workflows/ci.yml`](../../.github/workflows/ci.yml) wires:

The Phase 20g gate includes `corvid test rewrites`, so preserved-semantics drift is a CI failure with rule/law attribution rather than an optional local check.

- `cargo check --workspace --all-targets`
- `cargo test --workspace --lib --tests`
- `corvid test dimensions` (§6)
- `corvid test spec` (§7)
- `corvid test spec --meta` (§5)
- `corvid verify --corpus tests/corpus` (§1 + §3)

Any failure blocks the build. Shipped in commit `4d4944b`.

## 10. Summary

| Technique | Status | CI gate |
|---|---|---|
| Cross-tier differential verification (§1) | ✅ live, corpus + shrinker + blame | ✅ |
| Preserved-semantics fuzzing (§3) | 🔨 slice A shipped, slices B/C in progress | partial |
| Adversarial LLM generation (§2) | ✅ deterministic taxonomy + compiler classifier live; provider sampling can feed same harness later | partial |
| Bounty-fed regression corpus (§4) | ✅ seed corpus, meta-gate, bounty page, and issue template live | ✅ |
| Self-verifying verification (§5) | ✅ live | ✅ |
| Algebraic-law proptest (§6) | ✅ live, 10k cases per law | ✅ |
| Spec↔compiler sync (§7) | ✅ live | ✅ |

Six of seven techniques are production-grade and gated in CI. Adversarial generation now has a deterministic seed harness and classifier; live provider-backed sampling remains a follow-up because it depends on API budget and nondeterministic model output.

No other language's effect system has any of these. Corvid is the first.
