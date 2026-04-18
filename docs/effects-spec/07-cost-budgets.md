# 07 — Cost analysis and budgets

The invention: **the compiler proves, before the program runs, that a declared budget cannot be exceeded — across every path through the call graph.**

## 1. The `@budget` constraint

```corvid
# expect: skip
@budget($1.00)
agent planner(query: String) -> Plan:
    ...
```

A single-dimensional budget bounds `cost`. Multi-dimensional forms bound several resources at once:

```corvid
# expect: skip
@budget($1.00, tokens=50000, latency=2000)
agent interactive_lookup(query: String) -> Plan:
    ...
```

The checker proves each bound independently. Any bound violation fails compilation.

## 2. Worst-case path analysis

The checker doesn't estimate "average cost." It computes the **worst-case path** through the agent's body — the maximum cost over every branching choice and loop iteration:

- **Sequence.** Sum the costs of each step.
- **Branch.** Take the *maximum* of the branches (the compiler doesn't know which branch runs; the upper bound covers either).
- **Loop.** Multiply the body's cost by the loop's iteration count.
- **Static loop.** When the iteration count is statically determinable (a concrete integer literal for the range size), the multiplication is exact. Otherwise, the analyzer emits `CostWarningKind::UnboundedLoop` and marks the estimate `bounded: false`.

Unbounded loops don't fail the budget check — the analyzer is honest about what it can't prove. Instead, users see the warning and decide: add a static bound, use `@budget(…, unbounded=ok)` (when it ships), or accept the runtime check.

Implementation: `compute_worst_case_cost` in [../../crates/corvid-types/src/effects.rs](../../crates/corvid-types/src/effects.rs).

## 3. Cost tree

Every budget analysis produces a `CostTree` — a hierarchical breakdown showing which calls contribute what. The tree drives three user-facing surfaces:

### 3.1 Error messages

When a budget constraint fails, the error cites the cost-tree path that exceeds the bound:

```
error: effect constraint violated in agent `planner`: cost $1.50 exceeds budget $1.00
   path: planner -> refine (loop × 30 iterations @ $0.05) = $1.50
```

The path is a literal slice of the cost tree — it tells the user exactly where the spend is going.

### 3.2 REPL `:cost` command

```
>>> :cost planner
planner                             $1.50 / $1.00 (BUDGET EXCEEDED)
├── classify_request                $0.02
└── refine (loop × 30)              $1.50
    └── refine_step                 $0.05
```

The REPL renders the cost tree as a hierarchy so developers can drill into hot spots. Implementation: `render_cost_tree` in [effects.rs](../../crates/corvid-types/src/effects.rs).

### 3.3 `corvid effect-diff`

Comparing two revisions of an agent produces a per-dimension diff of the cost tree. See [02 § 9](./02-composition-algebra.md) and [../../crates/corvid-driver/src/effect_diff.rs](../../crates/corvid-driver/src/effect_diff.rs).

## 4. Multi-dimensional bounds

`@budget($1.00, tokens=50000, latency=2000)` declares three bounds: cost ≤ $1.00, tokens ≤ 50000, latency_ms ≤ 2000.

Each dimension composes with its own archetype (all Sum for these three). The checker computes the worst-case path per dimension; any bound being exceeded fails compilation. The error names *which* bound fired:

```
error: effect constraint violated in agent `planner`: tokens 75000 exceeds budget 50000
   path: planner -> summarize (called 15 times) @ 5000 tokens = 75000 tokens
```

## 5. Runtime termination

Compile-time bounds are worst-case upper bounds. At runtime, actual cost accumulates step-by-step, and users with **streaming prompts** get an additional safety net: mid-stream termination.

```corvid
# expect: skip
@budget($0.50)
agent fast_answer(query: String) -> Stream<String>:
    yield classify(query)       # $0.01
    yield from stream(query)    # could stream indefinitely
```

The runtime tracks live cumulative cost per emitted token. When cost crosses the declared budget, the stream is terminated and the caller receives `BudgetExceeded`. The stream does not try to spend past the bound and refund later; it stops.

This applies to all three Sum-archetype budget dimensions (cost, tokens, latency_ms). See [08 — Streaming effects](./08-streaming.md) for the streaming-specific mechanics.

## 6. Confidence as a budget dimension?

No. `confidence` composes by Min, not Sum. A `@min_confidence(C)` constraint is a *floor*, not an upper bound. The `@budget` form is reserved for accumulating resources. Confidence floors are their own constraint form ([06](./06-confidence-gates.md)).

## 7. What the cost tree doesn't model

Three scenarios produce pessimistic or approximate estimates:

1. **Unbounded loops.** Covered in §2 above. The warning surfaces; the constraint doesn't fire.
2. **Dynamic branch selection on data.** `if classify(x) == "cheap": cheap_path() else: expensive_path()` — the checker takes the max. If `cheap_path` runs 99% of the time in practice, the worst-case estimate overcounts.
3. **Recursion.** Corvid doesn't support unbounded recursion in agents for this reason — the cost estimate would be infinite. Bounded recursion (a depth parameter) models as a bounded loop.

The pessimism is sound — the compiler guarantees no budget violation at runtime. But it can reject programs that would have been fine in practice. `@budget($...)` is a contract, not a predictor.

## 8. Worked example

```corvid
# expect: compile
effect cheap:
    cost: $0.01

effect heavy:
    cost: $0.10

tool lookup(id: String) -> String uses cheap

prompt refine(text: String) -> String uses heavy:
    "Refine {text}"

agent planner(id: String) -> String:
    doc = lookup(id)
    result = refine(doc)
    return result
```

The compiler's trace:

1. `lookup(id)` → cost $0.01.
2. `refine(doc)` → cost $0.10.
3. Sequence → $0.01 + $0.10 = $0.11.
4. No `@budget` declared → no constraint fires. Agent compiles.

Add `@budget($0.05)`:

```corvid
# expect: skip
@budget($0.05)
agent planner(id: String) -> String:
    doc = lookup(id)
    result = refine(doc)
    return result
```

Step 4: $0.11 > $0.05 → `EffectConstraintViolation` with path `planner -> refine @ $0.10`.

## 9. Implementation references

- Cost tree: `CostTreeNode` in [../../crates/corvid-types/src/effects.rs](../../crates/corvid-types/src/effects.rs).
- Worst-case analysis: `compute_worst_case_cost` — same file.
- Render: `render_cost_tree`, same file.
- Runtime live tracking: `streaming cost gate` in [../../crates/corvid-vm/src/interp.rs](../../crates/corvid-vm/src/interp.rs).

## Next

[08 — Streaming effects](./08-streaming.md) — `Stream<T>`, backpressure, mid-stream termination, progressive structured types.
