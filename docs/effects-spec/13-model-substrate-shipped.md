# 13 — Typed model substrate: what shipped (Phase 20h)

This section is the **implementation reference** for the typed model substrate. [Section 09](./09-model-substrate.md) was a design preview written before the compiler understood any of it; this section documents what actually lives in the toolchain today, with examples that compile against the current parser, resolver, type checker, and IR.

Eight compiler-side slices shipped: A (model declarations) -> B (`requires:` capability routing) -> C (`route:` content-aware) -> D (jurisdiction / compliance / privacy dimensions) -> E (`progressive:` refinement) -> I (`rollout` A/B variants) -> F (`ensemble` voting syntax) -> G (`adversarial:` pipeline + stage-chaining contract). The runtime track is shipped too: B-rt (capability dispatch), C-rt (route dispatch), E-rt (progressive refinement), I-rt (A/B rollout), F-rt (ensemble voting), G-rt (adversarial execution), and H (`corvid routing-report`).

Implementation references throughout this section cite the specific module and function that executes each feature. The spec's `corvid test spec --meta` harness keeps these examples honest on every build.

## 13.1 Model declarations (slice A)

A project declares its available models as top-level `model` blocks. Each field is `<name>: <DimensionValue>` — any dimension the effect system knows about (built-in or custom via [invention #6 corvid.toml](./01-dimensional-syntax.md)) is a valid field name.

```corvid
# expect: compile
model haiku:
    cost_per_token_in: $0.00000025
    cost_per_token_out: $0.00000125
    capability: basic
    latency: fast

model opus:
    cost_per_token_in: $0.000015
    capability: expert
    latency: normal
```

The parser's `parse_model_decl` (see [crates/corvid-syntax/src/parser.rs](../../crates/corvid-syntax/src/parser.rs)) produces `ModelDecl { name, fields, span }`. The resolver registers the model name under `DeclKind::Model` — duplicate models, or name collisions with tools / prompts / agents / effects, fail to resolve with `DuplicateDecl`.

**Inventive angle.** Field names are not hardcoded. A user who declares a custom dimension `fairness` via `corvid.toml` can freely write `fairness: 0.92` inside a model block. The catalog is user-extensible in the same way the effect algebra is.

## 13.2 Capability-based routing (slice B)

A prompt declares a minimum capability requirement. The runtime picks the cheapest model that satisfies it.

```corvid
# expect: compile
model haiku:
    capability: basic

model opus:
    capability: expert

prompt classify(t: String) -> String:
    requires: basic
    "Classify {t}"

prompt legal_analysis(t: String) -> String:
    requires: expert
    "Analyze {t} for precedent"
```

`capability` is the eighth built-in dimension — Max-composed over the `basic < standard < expert` lattice with identity `basic`. Unknown capability names (a user-declared `frontier` tier, for example) rank above `expert` so the strictest-wins invariant survives arbitrary lattice extensions.

Agent bodies that call multiple prompts compose the capability requirement through the full call graph. An agent that calls one `basic` prompt and one `expert` prompt has composed capability `expert`:

```corvid
# expect: compile
model haiku:
    capability: basic

model opus:
    capability: expert

prompt simple(t: String) -> String:
    requires: basic
    "Simple {t}"

prompt hard(t: String) -> String:
    requires: expert
    "Hard {t}"

agent triage(t: String) -> String:
    a = simple(t)
    b = hard(t)
    return b
```

Implementation: `PromptDecl.capability_required` in [crates/corvid-ast/src/decl.rs](../../crates/corvid-ast/src/decl.rs); `IrPrompt.capability_required` in [crates/corvid-ir/src/types.rs](../../crates/corvid-ir/src/types.rs); `collect_body_capabilities` in [crates/corvid-types/src/effects.rs](../../crates/corvid-types/src/effects.rs) walks agent bodies and Max-composes per-prompt requirements into the agent's composed profile.

## 13.3 Content-aware routing (slice C)

A prompt can pattern-dispatch to different models per call. Each arm pairs a boolean guard (or the `_` wildcard) with a model reference.

```corvid
# expect: compile
model fast:
    capability: basic

model slow:
    capability: expert

prompt answer(q: String) -> String:
    route:
        q == "hard" -> slow
        _ -> fast
    "Answer {q}"
```

Arms evaluate top-to-bottom; first match wins. The wildcard catches anything not matched above. Guard expressions can reference the prompt's parameters — `q` here is the prompt's own input. Any boolean-valued expression is a valid guard: comparisons, classifier-function calls, boolean combinators.

Validation the checker performs:

- **Guard type**: each guard must typecheck to `Bool`. Non-Bool guards produce `RouteGuardNotBool`.
- **Model reference**: each arm's right-hand side must bind to a `Decl::Model`. Pointing at a tool, prompt, agent, or effect produces `RouteTargetNotModel` naming the wrong kind.
- **Undefined names**: unresolved model names are rejected by the resolver with `UndefinedName`.

Implementation: `RouteTable`, `RouteArm`, `RoutePattern` in [crates/corvid-ast/src/decl.rs](../../crates/corvid-ast/src/decl.rs); `parse_prompt_route_block` in [crates/corvid-syntax/src/parser.rs](../../crates/corvid-syntax/src/parser.rs); `IrPrompt.route: Vec<IrRouteArm>` carries resolved `DefId`s into the runtime.

**Inventive angle.** Classifier functions (`domain(q)`, `language(q)`, `length(q)`, `is_image(q)`, …) are **not hardcoded**. Any boolean expression over the prompt's inputs is valid. Users who need custom classifiers declare them as tools or prompts and route on them — the language ships the dispatch primitive, not a fixed classifier vocabulary.

## 13.4 Regulatory / compliance / privacy dimensions (slice D)

Three additional built-in dimensions land on the model catalog:

| Dimension | Archetype | Default | Composer |
|---|---|---|---|
| `jurisdiction` | Max | `none` | lexicographic fallback for unknown pairs |
| `compliance` | Union | `none` | set-union over comma-separated tags |
| `privacy_tier` | Max | `standard` | `standard < strict < air_gapped` |

These dimensions work identically to the pre-existing built-ins (`cost`, `trust`, `reversible`, `data`, `latency`, `confidence`, `tokens`, `latency_ms`, `capability`) — `corvid test dimensions` verifies the full set of eleven on every CI run.

Example catalog using the regulatory dimensions:

```corvid
# expect: compile
model claude_hipaa:
    jurisdiction: us_hipaa_bva
    compliance: hipaa
    privacy_tier: strict
    capability: expert

model claude_eu:
    jurisdiction: eu_hosted
    compliance: gdpr
    privacy_tier: strict
    capability: expert

model haiku:
    jurisdiction: us_hosted
    privacy_tier: standard
    capability: basic
```

At runtime (Dev B's B-rt and subsequent slices), model eligibility tests intersect every required dimension — a prompt that needs `jurisdiction: us_hipaa_bva` and `compliance: hipaa` gets only models satisfying both. Slice B shipped the capability gate; slice D shipped the dimension plumbing; the multi-dimension gate lands when B-rt's sibling dispatch slices ship.

Two latent bugs the law-check harness caught during slice D:

- `trust_max` returned `a` unconditionally on tied ranks — violating commutativity when two unknown-to-trust-lattice names (like two jurisdiction tags) compared. Fixed with lex tie-break.
- `trust_max` lost the `"none"` identity when a tag lexicographically followed it (e.g. `'e' < 'n'` made `trust_max("eu_hosted", "none") = "none"`). Fixed by absorbing `"none"` before the lattice lookup.

Both fixes shipped in commit `b88307a`. `corvid test dimensions` now reports **all 11 dimensions satisfy their archetype's laws**.

## 13.5 Progressive refinement (slice E)

A prompt can declare a linear chain of models. The runtime runs stages in order, accepting the first stage whose output confidence meets the declared threshold and escalating otherwise.

```corvid
# expect: compile
model cheap:
    capability: basic

model medium:
    capability: standard

model expensive:
    capability: expert

prompt classify(q: String) -> String:
    progressive:
        cheap below 0.95
        medium below 0.99
        expensive
    "Classify {q}"
```

Semantics:

1. Run `cheap`.
2. If its output confidence ≥ 0.95, return the result.
3. Else run `medium`.
4. If confidence ≥ 0.99, return.
5. Else run `expensive` — the terminal stage that always returns.

The grammar enforces the contract at parse time:

- At least two stages (primary + terminal fallback). Single-stage `progressive:` blocks are rejected.
- Every non-terminal stage must declare `below <threshold>`.
- The terminal stage must NOT declare a threshold — it's the unconditional fallback.

Mutually exclusive with `route:` on the same prompt. The parser rejects the combination with a message pointing at the exclusive-dispatch rule.

Validation the checker adds on top:
- Each stage's model ident must resolve to a `Decl::Model`.
- Each threshold must fall in `[0.0, 1.0]`. Out-of-range thresholds fire `InvalidConfidence`.

Implementation: `ProgressiveChain`, `ProgressiveStage` in [crates/corvid-ast/src/decl.rs](../../crates/corvid-ast/src/decl.rs); `parse_prompt_progressive_block` in the parser; `IrProgressiveStage` in [crates/corvid-ir/src/types.rs](../../crates/corvid-ir/src/types.rs).

**Inventive angle.** Thresholds use the same `confidence` dimension the effect system already tracks. No new type. That means a future slice can extend `below` to reference other numeric dimensions (e.g. `below cost` for cost-based escalation) without grammar changes — the escalation direction is a dimension-level property.

## 13.6 A/B rollouts (slice I)

A one-line variant dispatch: send a percentage of calls to a new model, the rest to the baseline.

```corvid
# expect: compile
model opus_v1:
    capability: expert

model opus_v2:
    capability: expert

prompt summarize(doc: String) -> String:
    rollout 10% opus_v2, else opus_v1
    "Summarize {doc}"
```

Grammar: `rollout <N>% <variant_ident>, else <baseline_ident>`. Percentage accepts integer or float literals (`10%` or `2.5%`).

Mutually exclusive with both `route:` and `progressive:` — a prompt uses exactly one dispatch strategy. The parser rejects the combination with the exclusive-dispatch message.

Validation:
- Both variant and baseline must be `Decl::Model`s.
- `variant_percent` must fall in `[0.0, 100.0]`. Out-of-range triggers `RolloutPercentOutOfRange`.

Runtime cohort-assignment strategy (uniform random, session-sticky, deterministic-per-call-id) is Dev B's I-rt decision — the compiler-side contract is just the two model references and the percentage.

Implementation: `RolloutSpec` in [crates/corvid-ast/src/decl.rs](../../crates/corvid-ast/src/decl.rs); `parse_prompt_rollout_clause` in the parser; `IrRolloutSpec` in [crates/corvid-ir/src/types.rs](../../crates/corvid-ir/src/types.rs).

## 13.7 Ensemble voting (slice F)

Dispatch a prompt to several models concurrently and fold the results with a vote. The compiler-side shape is a model list and a vote strategy; the shipped runtime fires every member concurrently and emits `TraceEvent::EnsembleVote` with the full vote record.

```corvid
# expect: compile
model opus:
    capability: expert

model sonnet:
    capability: expert

model haiku:
    capability: standard

prompt classify(q: String) -> String:
    ensemble [opus, sonnet, haiku] vote majority
    "Classify {q}"
```

Grammar: `ensemble [<model>, <model>, ...] vote <strategy>`. Slice F ships `majority`; weighted and unanimous votes are reserved for a follow-up.

Validation:
- Every ensemble member must resolve to a `Decl::Model` (`RouteTargetNotModel`).
- Duplicate models fire `EnsembleDuplicateModel` -> voting with two slots on the same provider-model pair degenerates silently to voting over fewer opinions, which is almost always unintended.

Runtime semantics:
- All listed models dispatch concurrently.
- `majority` picks the plurality string output; ties break alphabetically on output text for determinism.
- Cost composes as **Sum** across all members.
- Confidence composes as **Min(member confidences) * agreement_rate** where `agreement_rate = winner_votes / total_votes`.
- Every call emits `TraceEvent::EnsembleVote { prompt, members, results, winner, agreement_rate, strategy }`.

Implementation: `EnsembleSpec`, `VoteStrategy` in [crates/corvid-ast/src/decl.rs](../../crates/corvid-ast/src/decl.rs); `parse_prompt_ensemble_clause` in the parser; `IrEnsembleSpec`, `IrEnsembleMember`, `IrVoteStrategy` in [crates/corvid-ir/src/types.rs](../../crates/corvid-ir/src/types.rs); runtime execution in `corvid-vm` + [crates/corvid-runtime/src/ensemble.rs](../../crates/corvid-runtime/src/ensemble.rs).

## 13.8 Adversarial validation (slice G)

Three-stage pipeline: a **proposer** produces a candidate, a **challenger** critiques it, and an **adjudicator** reads both and returns the final verdict. Each stage is a `prompt` decl -> not a bare model -> because stages chain outputs as positional arguments into the next stage.

```corvid
# expect: compile
type Verdict:
    contradiction: Bool
    rationale: String

prompt propose_answer(q: String) -> String:
    "Answer: {q}"

prompt critique(proposed: String) -> String:
    "Find flaws in: {proposed}"

prompt adjudicate_fn(proposed: String, flaws: String) -> Verdict:
    "Given {proposed} and the critique, verdict?"

prompt verify(q: String) -> Verdict:
    adversarial:
        propose: propose_answer
        challenge: critique
        adjudicate: adjudicate_fn
    "Verify"
```

Grammar: `adversarial:` followed by exactly three stages in canonical order -> `propose: <prompt>`, `challenge: <prompt>`, `adjudicate: <prompt>`. Stages out of order, missing stages, or stages pointing at non-prompt decls all fail parse or check.

**Stage chaining contract** (enforced in the checker):

| Stage | Arity | Param types | Return type |
|---|---|---|---|
| `propose` | = outer prompt's arity | must accept the outer prompt's param types | `T1` (free) |
| `challenge` | exactly 1 | must accept `T1` | `T2` (free) |
| `adjudicate` | exactly 2 | must accept `(T1, T2)` | must match the outer prompt's return type **and** be a struct with a `contradiction: Bool` field |

**Why stages are prompts, not models.** Models are passive capability descriptors -> the runtime dispatches _to_ a model. An adversarial pipeline needs callable units with typed signatures so the compiler can enforce the chaining contract: each stage's return type flows as the next stage's input. A bare model reference has no signature the compiler can type-check against. Prompts do.

**Why `contradiction: Bool` is required.** The runtime emits `TraceEvent::AdversarialContradiction` when a pipeline detects a flaw the adjudicator flags. The only way to make that event reachable by construction -> rather than a soft convention that sometimes fires -> is to require the adjudicator's return struct to carry the field the runtime reads. This is the same design as `approve`-before-`dangerous` and `cites strictly` elsewhere: make the compiler enforce the contract the runtime depends on.

**Mutual exclusion.** Like every dispatch clause, `adversarial:` is mutually exclusive with `route:`, `progressive:`, `rollout`, and `ensemble` -> the parser rejects any two on the same prompt pointing at the second clause.

Validation errors specific to adversarial:
- `AdversarialStageNotPrompt` -> a stage resolved to a non-prompt decl (model, tool, agent, type, etc.).
- `AdversarialStageArity` -> wrong parameter count for the stage's role.
- `AdversarialStageParamType` -> a stage's parameter type can't accept the previous stage's output (or the outer prompt's param for the proposer).
- `AdversarialStageReturnType` -> adjudicator's return type doesn't match the outer prompt's return type.
- `AdversarialAdjudicatorMissingContradictionField` -> adjudicator's return is not a struct, or its struct has no `contradiction: Bool` field.

Runtime semantics:
- The pipeline runs **sequentially**: `propose(outer_args)` -> `challenge(proposed)` -> `adjudicate(proposed, challenge)`.
- Stages are ordinary prompt calls, so each stage reuses the standard dispatch path (capability / route / progressive / rollout / ensemble) and composes its own cost, confidence, and latency through the existing prompt executor.
- Pipeline cost composes as **Sum** across the three stages; confidence composes as **Min** across the three stage results.
- When the adjudicator's `contradiction: Bool` field is `true`, the runtime emits `TraceEvent::AdversarialContradiction { prompt, proposed, challenge, verdict }`.
- Every completed pipeline emits `TraceEvent::AdversarialPipelineCompleted { prompt, contradiction }` whether or not a contradiction was found.

Implementation: `AdversarialSpec` in [crates/corvid-ast/src/decl.rs](../../crates/corvid-ast/src/decl.rs); `parse_prompt_adversarial_clause` + `parse_adversarial_stage` in the parser; `IrAdversarialSpec` in [crates/corvid-ir/src/types.rs](../../crates/corvid-ir/src/types.rs); contract enforcement in `check_prompt` in [crates/corvid-types/src/checker.rs](../../crates/corvid-types/src/checker.rs); runtime helpers in [crates/corvid-runtime/src/adversarial.rs](../../crates/corvid-runtime/src/adversarial.rs).

## 13.9 Dispatch strategies: one per prompt

A prompt declares **at most one** dispatch clause: `route:`, `progressive:`, `rollout`, `ensemble`, or `adversarial:`. Without any, the prompt uses the default capability-based dispatch (slice B).

The parser enforces mutual exclusion with targeted errors. Combining any two on the same prompt fires a "a prompt uses exactly one of `route:`, `progressive:`, `rollout`, `ensemble`, or `adversarial:`" message pointing at the second clause.

## 13.10 Operational status

All six dispatch strategies are now shipped end to end: capability-based dispatch, `route:`, `progressive:`, `rollout`, `ensemble`, and `adversarial:`.

The `corvid routing-report` CLI is shipped too. It aggregates `ModelSelected`, `ProgressiveEscalation`, `ProgressiveExhausted`, `AbVariantChosen`, `EnsembleVote`, `AdversarialPipelineCompleted`, and `AdversarialContradiction` into three operational tables:
- per-prompt model usage
- escalation patterns
- A/B + ensemble + adversarial strategy outcomes

The report is intentionally prescriptive: every row carries a recommendation rather than just raw counts.

## 13.11 Verification status

Every slice passes three CI gates on every push:

- **`corvid test dimensions`** — 11 dimensions × archetype laws × 10,000 cases each.
- **`corvid test spec`** — every ```corvid fenced block in this section (and every other spec section) compiles as claimed.
- **`corvid test spec --meta`** — every historical counter-example in [counterexamples/composition/](./counterexamples/composition/) still distinguishes its correct composition rule from the attacker's wrong rule.

Plus `cargo test --workspace` across the full suite. The verifier corpus at [tests/corpus/](../../tests/corpus/) cross-verifies every fixture across four execution tiers; deliberate-fail fixtures (`should_fail/tier_disagree.cor`, `should_fail/native_drops_effect.cor`) prove the harness catches divergences rather than waving them through.

## 13.12 Shipping trail

| Slice | Commit | What shipped |
|---|---|---|
| A | `59b8663` | Model declarations + parser + resolver namespace |
| B | `56253d4` | `requires:` capability clause + Max composition through call graph |
| C | `0da3efc` | `route:` pattern dispatch + Bool-guard validation + Model-ref validation |
| D | `b88307a` | jurisdiction / compliance / privacy_tier dimensions + two trust_max bug fixes |
| E | `6accbc2` | `progressive:` chain + stage-terminal-fallback grammar + threshold range check |
| I | `e1476c3` | `rollout N%` one-liner + mutual-exclusion rejection with route/progressive |
| F (syntax) | `171b68f` | `ensemble [...]  vote majority` + duplicate-model rejection |
| G (syntax) | `6047e00` | `adversarial:` propose / challenge / adjudicate block + order / arity parse checks |
| B-rt | `a2b9160` | Runtime: capability-based model dispatch (Dev B) |
| C-rt | `cf301d7` | Runtime: content-aware route dispatch (Dev B) |
| E-rt | `1722a7a` | Runtime: progressive escalation (Dev B) |
| I-rt | `04f5c77` | Runtime: rollout dispatch with seeded PRNG + `AbVariantChosen` trace (Dev B) |
| F-rt | `7651420` | Runtime: ensemble concurrent dispatch + voting + `EnsembleVote` trace (Dev B) |
| G (contract) | `a0345e7` | Stage resolution switched to prompts; chaining contract + `contradiction: Bool` field check |
| G-rt | `a610894` | Runtime: adversarial sequential pipeline + contradiction traces (Dev B) |
| H | `24c56fa` | `corvid routing-report` CLI + routing trace aggregation (Dev B) |

## Next

[11 — Related work](./11-related-work.md) compares Corvid's dispatch strategies against LangChain / OpenRouter / Portkey. [09 — Model substrate (design preview)](./09-model-substrate.md) covers the full 20h design as the original design reference now that the full runtime track is shipped.
