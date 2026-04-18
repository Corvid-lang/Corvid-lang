# 09 — Typed model substrate (Phase 20h preview)

**This section previews Phase 20h — the typed compute substrate for AI models. The mechanism is specified here for design-review purposes; shipping implementation lands in 20h.**

The conceptual leap: Corvid doesn't just *call* LLMs. It provides a **typed compute substrate** for models, where each model is a typed resource with declared capabilities, and the language proves regulatory / cost / quality properties at the type level.

No other language or framework has any of this. LangChain has manual fallback chains. OpenRouter has cloud routing. Portkey has a gateway. None of them treat the LLM ecosystem as a typed substrate with compile-time guarantees.

## 1. Model catalog

A project declares its available models. Each model carries a dimensional profile:

```corvid
# expect: skip
model haiku:
    cost_per_token_in: $0.00000025
    cost_per_token_out: $0.00000125
    capability: basic
    latency: fast
    max_context: 200000
    jurisdiction: us_hosted
    privacy_tier: standard

model opus:
    cost_per_token_in: $0.000015
    capability: expert

model deepseek_math:
    specialty: math
    capability: standard

model claude_hipaa:
    jurisdiction: us_hipaa_bva
    compliance: [hipaa]
    privacy_tier: strict
```

The catalog lives in the project's corvid.toml (or a dedicated `models.cor` file in 20h). Each `model` declaration registers a typed resource.

## 2. Capability-based routing

Prompts declare *requirements*, not models. The runtime picks the cheapest model that qualifies:

```corvid
# expect: skip
prompt classify(t: Ticket) -> Category:
    requires: basic
    latency: fast
    "Classify {t}."
    # runtime picks haiku

prompt legal_analysis(c: Case) -> Analysis:
    requires: expert
    "Analyze {c} for precedent."
    # runtime picks opus (or another `expert` model)
```

Capability composes through the call graph via `Max`. An agent calling three prompts at `basic` / `standard` / `expert` has composed requirement `expert`. The compiler proves it.

## 3. Content-aware routing

Pattern-match the prompt's input to select the model:

```corvid
# expect: skip
prompt answer(question: String) -> Answer:
    route:
        domain(question) == math      -> deepseek_math
        language(question) == dutch   -> gpt_oss_dutch
        length(question) > 50000      -> claude_long
        _                             -> gpt4
    "Answer {question}."
```

The router is a typed dispatch function. `domain`, `language`, `length` etc. are compiled as classifier prompts (cheap models) that produce typed values for pattern matching.

## 4. Jurisdiction and compliance

Regulatory properties are first-class dimensions:

```corvid
# expect: skip
prompt analyze_medical_record(r: Record) -> Finding:
    requires jurisdiction: us_hipaa_bva
    requires compliance: [hipaa]
    "..."
```

The runtime's model selection respects these requirements. If no available model satisfies `us_hipaa_bva` with `hipaa` compliance, the call fails at startup, not at request time.

## 5. Privacy tiers

```corvid
# expect: skip
prompt process_pii(data: String) -> Summary:
    requires privacy_tier: strict
    "..."
```

Models tagged `privacy_tier: strict` (on-prem, zero-retention, SOC2) can serve; cheaper models cannot. The catalog's `privacy_tier` declaration gates eligibility.

## 6. Progressive refinement

When a cheaper model's output is uncertain, fall through to a more expensive one:

```corvid
# expect: skip
prompt classify_with_refinement(t: Ticket) -> Category:
    route:
        classify_cheap(t).confidence >= 0.95 -> classify_cheap
        _                                    -> classify_expensive
    "..."
```

The runtime invokes `classify_cheap` first. If confidence is high enough, use it. Otherwise, escalate to `classify_expensive`. Compile-time checking: the composed confidence is still bounded by Min.

## 7. Ensemble voting

```corvid
# expect: skip
prompt high_confidence_answer(q: String) -> Answer:
    ensemble [haiku, sonnet, opus] vote majority
    "Answer {q}."
```

Three models vote; the majority wins. Runtime semantics: fire three concurrent calls, wait for all (or a majority threshold), return the plurality answer. Cost composes as the sum of all three model costs. Confidence composes as the *agreement rate* — a novel per-prompt dimension.

## 8. Adversarial validation

```corvid
# expect: skip
prompt verified_claim(q: String) -> Claim:
    propose: opus "Answer {q}"
    challenge: sonnet "Find flaws in this answer: {proposed}"
    adjudicate: opus "Given these challenges, revise: {proposed} {challenge}"
```

Three stages, potentially different models. The compiler types the pipeline; the cost is summed across all three stages; the output is the adjudicated final claim.

## 9. Cost-frontier visualization

`corvid routing-report` analyzes per-prompt routing decisions and their cost/quality trade-offs:

```
>>> corvid routing-report --since 7d
summarize       chose haiku (97%) / opus (3%) — avg $0.002
classify        chose haiku (100%) — avg $0.0005
legal_answer    chose opus (100%) — avg $0.08

underutilized models: sonnet (always beaten by haiku on cost, always beaten by opus on quality)
over-routed: claude_long (routed 12%, but average input fits in 100k — consider raising length threshold)
```

The report points at routing rules worth tuning. This is the operational counterpart to the compile-time guarantees — the compiler proves "calls are legal"; the report shows "are they optimal?"

## 10. A/B rollouts

```corvid
# expect: skip
prompt summarize(doc: String) -> String:
    rollout 10% opus_v2, else opus_v1
    "Summarize {doc}"
```

Deploys the new model gradually. The runtime's routing decision is recorded in the trace so `corvid eval` can compare outcomes between variants.

## 11. Interaction with existing dimensions

Model substrate adds new dimensions to the effect system. They compose via the standard archetypes:

| New dimension | Archetype | Notes |
|---|---|---|
| `capability` | Max | standard < expert; composed is the strictest requirement |
| `jurisdiction` | Max (lattice) | Regional regulatory zones form a total order by strictness |
| `compliance` | Union | Set of compliance tags accumulated across the chain |
| `privacy_tier` | Max (lattice) | `standard < strict < air_gapped` |
| `specialty` | Union | Specialty tags combine — `math + language(dutch)` |

All five are user-declarable as custom dimensions via corvid.toml even before 20h ships, since custom dimensions are already in 20g invention #6. The routing DSL is the new surface area 20h adds.

## 12. What's shipping when

20h's shipping scope:
- `model` declaration syntax + dimension catalog (commit sequence TBD).
- Capability-based routing at the prompt level.
- Content-aware `route:` clause with pattern matching.
- Registry of built-in capability tags.
- `corvid routing-report` CLI.

Later phases:
- Ensemble voting + adversarial validation (20i research).
- Progressive refinement syntactic sugar (20i).
- A/B rollouts (20j — evaluation phase).
- Cost-frontier visualization richer than the single table above.

## 13. Why this is new

LangChain has manual fallback chains — the programmer writes `if model1.fails { model2 } else { model1 }`. OpenRouter has cloud routing — fire-and-forget, no guarantees. Portkey has a gateway — routes requests by rule, no type-system integration. None of them let the language type-check your routing decisions or prove that your jurisdiction constraints hold on every path.

Corvid's model substrate is the first time a programming language treats the LLM ecosystem as a typed resource pool.

## Next

[10 — FFI, generics, async interactions](./10-interactions.md) — how the effect system composes across language boundaries.
