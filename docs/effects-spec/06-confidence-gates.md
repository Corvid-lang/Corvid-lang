# 06 — Confidence-gated trust

The invention: **the safety boundary of a tool adapts to how confident the AI is on this specific call.**

A tool declared `trust: autonomous_if_confident(0.95)` compiles as if it were `autonomous` — no approval gate at static checking time. At runtime, the interpreter inspects the composed confidence of the tool's inputs. If confidence has dropped below 0.95 on this invocation, the runtime activates the approval gate and requires human confirmation. If confidence is above 0.95, the call proceeds.

No other language has this two-layer trust model. Every other effect system treats authorization as a flat property: either human approval is required or it isn't. Corvid makes the boundary a function of *runtime confidence*.

## 1. The declaration

```corvid
# expect: skip
effect high_stakes:
    trust: autonomous_if_confident(0.95)
    reversible: false

tool auto_approve_refund(id: String, amount: Float) -> Receipt uses high_stakes
```

The `ConfidenceGated { threshold: 0.95, above: "autonomous", below: "human_required" }` variant on `DimensionValue::Trust` carries three pieces:

- **threshold** — the confidence floor below which the gate activates.
- **above** — the trust level applied when confidence ≥ threshold.
- **below** — the trust level applied when confidence < threshold.

Compile time sees `above` (`autonomous`). Runtime sees `below` (`human_required`) when confidence falls short.

## 2. Compile-time composition

`autonomous_if_confident(T)` composes with other trust values under Max just like any `Name` trust level. The composition uses `above` for static comparison:

- `autonomous` ⊕ `autonomous_if_confident(0.95)` → `autonomous_if_confident(0.95)` (above is autonomous, autonomous stays)
- `human_required` ⊕ `autonomous_if_confident(0.95)` → `human_required` (human_required dominates)
- `autonomous_if_confident(0.95)` ⊕ `autonomous_if_confident(0.80)` → `autonomous_if_confident(0.95)` (stricter threshold wins)

Implementation: `compose_max_dimension` in [effects.rs](../../crates/corvid-types/src/effects.rs) has a dedicated arm for `ConfidenceGated` that threads the threshold through.

An agent declared `@trust(autonomous)` can compose with a `ConfidenceGated(0.95)` tool and still compile — statically, the composed level is `autonomous`. The gate fires only at runtime.

## 3. Runtime gate activation

Every tool call's trust value is materialized at runtime. The interpreter, before dispatching a `ConfidenceGated` tool call:

1. Computes **composed input confidence** — `min(confidence(arg₁), …, confidence(argₙ))`. Arguments without `confidence` metadata contribute `1.0`.
2. Compares to the tool's threshold `T`.
3. If composed ≥ T: proceed autonomously.
4. If composed < T: raise `ApprovalRequired { threshold: T, observed: composed }`.

The caller (REPL, runtime approver hook, test harness) handles the approval. See `composed_confidence` in [../../crates/corvid-vm/src/interp.rs](../../crates/corvid-vm/src/interp.rs) and the `IrTool::confidence_gate` field for the plumbing.

## 4. `@min_confidence` as a static counterpart

Confidence-gated trust is a *per-call* runtime mechanism. `@min_confidence(C)` on an agent is the static counterpart — it asserts that *every* call in the agent's body will produce a value with confidence ≥ C.

```corvid
# expect: skip
@trust(autonomous)
@min_confidence(0.95)
agent confident_refund(id: String, amount: Float) -> Receipt:
    approve AutoApproveRefund(id, amount)
    return auto_approve_refund(id, amount)
```

If `@min_confidence(0.95)` is declared and the body's composed confidence is 0.90, compilation fails. With the declaration, the programmer is promising the confidence floor; the checker proves the promise.

`@min_confidence` and `autonomous_if_confident` typically pair:
- Declare the tool with `autonomous_if_confident(T)`.
- Declare the agent with `@min_confidence(T)` matching the tool's threshold.
- Compile-time proof: all inputs to the tool are confidence ≥ T.
- Runtime reality: the gate doesn't activate because the compile-time proof holds.

Without `@min_confidence`, the runtime gate is the only safety net. With it, the gate becomes an assertion that the compiler has already proved.

## 5. Why this is new

Other systems' trust models:

| System | Trust model |
|---|---|
| LangChain | Flat tool-permission lists. No confidence integration. |
| OpenAI function calling | Flat function allow-list. |
| Capability-based security | Static capability possession. |
| HIPAA workflow tools | Hard-coded approval requirements. |

None of these let authorization depend on the **current statistical confidence** of the operation. Corvid binds the two dimensions: trust and confidence compose statically through separate rules (Max, Min respectively), and at runtime the confidence value activates or relaxes the trust gate.

This enables programs like:

- "Refund up to $100 without approval if model confidence ≥ 0.95, require human otherwise."
- "Send email autonomously when classification confidence ≥ 0.90, otherwise queue for review."
- "Run the legal-research pipeline without oversight when every step returned confidence ≥ 0.99, otherwise flag for attorney."

Each of these is expressible in Corvid as a single tool declaration + single agent annotation, and the language proves the rest.

## 6. Worked example

```corvid
# expect: skip
effect refund_effect:
    cost: $0.00
    trust: autonomous_if_confident(0.90)
    reversible: false

tool issue_refund(id: String, amount: Float) -> Receipt uses refund_effect

prompt classify_request(text: String) -> String:
    "Classify: {text}"

@trust(autonomous)
agent auto_refund_bot(text: String, id: String, amount: Float) -> Receipt:
    kind = classify_request(text)
    approve IssueRefund(id, amount)
    return issue_refund(id, amount)
```

Compile-time:
- `classify_request` returns a plain `String` with unknown confidence (default 1.0 in absence of annotations).
- `issue_refund`'s trust composes as `autonomous_if_confident(0.90)`; static composition gives the agent trust level `autonomous_if_confident(0.90)`.
- `@trust(autonomous)` — the checker treats the gated level as `autonomous` for this comparison. Compiles.

Runtime invocation 1: classify_request returns with confidence 0.95. The gate doesn't fire. The refund issues autonomously.

Runtime invocation 2: classify_request returns with confidence 0.75. The gate fires. The runtime raises `ApprovalRequired { threshold: 0.90, observed: 0.75 }`. The caller's approver hook handles it — prompts the human, times out, or auto-rejects per policy.

## 7. Implementation references

- AST variant: `DimensionValue::ConfidenceGated` in [../../crates/corvid-ast/src/effect.rs](../../crates/corvid-ast/src/effect.rs).
- IR field: `IrTool::confidence_gate: Option<f64>` in [../../crates/corvid-ir/src/types.rs](../../crates/corvid-ir/src/types.rs).
- Runtime check: `composed_confidence` in [../../crates/corvid-vm/src/interp.rs](../../crates/corvid-vm/src/interp.rs).
- Error type: `ApprovalRequired` / `ApprovalDenied` in the VM's error enum.

## Next

[07 — Cost analysis and budgets](./07-cost-budgets.md) — multi-dimensional `@budget`, worst-case path analysis, cost-tree visualization, and the `:cost` REPL command.
