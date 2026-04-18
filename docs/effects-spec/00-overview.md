# 00 — Overview and motivation

## What problem does the effect system solve?

AI agents take irreversible actions with borrowed authority. They call tools that move money, delete data, send emails. They return answers that may or may not be grounded in retrieved context. They spend money per token. They take wall-clock time that users wait for. They make decisions at confidence levels the caller cannot see.

No general-purpose language has a type system that treats any of these concerns as a first-class property. Python, TypeScript, Rust, Go were designed before AI agents were a programming pattern. Their type systems reason about `Int`, `List<T>`, `Result<T, E>` — not about whether a function can move money without human approval, whether its output is grounded in retrieved context, whether its composed worst-case cost exceeds a budget, or whether its composed confidence is high enough for an autonomous decision.

Corvid's dimensional effect system is the language-level answer.

## The core idea: effects carry typed dimensions that compose independently

A Corvid `effect` declaration is not a flat tag. It is a bundle of **typed dimensions** — measurable, named properties of the effect:

```corvid
effect transfer_money:
    cost: $0.001
    reversible: false
    trust: human_required
    data: financial
    latency: fast
```

When effects combine through a call graph, each dimension composes independently via its own rule:

| Dimension | Composition rule | Example |
|---|---|---|
| `cost` | Sum | `$0.001 + $0.003 + $0.015 = $0.019` along the path |
| `trust` | Max (most restrictive wins) | `autonomous` + `human_required` = `human_required` |
| `reversible` | AND (least reversible wins) | `true` + `false` = `false` (once anything is irreversible, the chain is) |
| `data` | Union | `financial` + `medical` = `financial, medical` |
| `latency` | Max | `fast` + `slow` = `slow` |
| `confidence` | Min (weakest link) | `0.95` composed with `0.75` = `0.75` |

This is the observation no other language has made: **the different safety concerns of AI agent code compose with different rules, and the compiler can enforce each rule independently.**

## What this enables at compile time

The compiler rejects programs that violate dimensional constraints:

```corvid
@trust(autonomous)
agent fast_lookup(query: String) -> String:
    result = search_knowledge(query)    # trust: autonomous — OK
    transfer_money(result)              # trust: human_required — COMPILE ERROR
                                        # agent `fast_lookup` declares @trust(autonomous)
                                        # but composed trust is human_required
```

```corvid
@budget($1.00)
agent planner(query: String) -> Plan:
    for item in expensive_list:         # 34 iterations of a $0.030 prompt
        refine(item)
                                        # COMPILE ERROR
                                        # worst-case cost $1.031 > $1.00 budget
                                        # path: refine × 34 iterations
```

```corvid
agent researcher(query: String) -> Grounded<String>:
    answer = hallucinate(query)         # no grounded source in chain
    return answer                       # COMPILE ERROR
                                        # Grounded<String> return requires at
                                        # least one `data: grounded` source in
                                        # the provenance chain
```

## What this enables at runtime

The effect system is not purely static. Some dimensions gain dynamic behavior:

**Confidence-gated trust.** A tool declared `trust: autonomous_if_confident(0.95)` runs autonomously at compile time (the compiler treats it as `autonomous`), but at runtime the interpreter checks the composed confidence of the tool's inputs. If confidence has dropped below 0.95, the runtime activates the approval gate and requires human confirmation. **The safety boundary adapts to how confident the AI is on this specific call.**

**Provenance chains.** Every `Grounded<T>` value carries runtime metadata — the retrieval sources, prompt transformations, and agent handoffs the value passed through. The `.sources()` method returns the chain. The `cites ctx strictly` verifier walks the chain at runtime and confirms the LLM's output references the specific context it was supposed to.

**Budget termination mid-stream.** When `@budget($1.00)` is active and an agent calls a streaming prompt, the cost dimension is tracked live per token. If cumulative cost crosses the budget while the stream is producing, the runtime terminates the stream and raises `BudgetExceeded`.

## What this enables for developer tooling

The effect profile is a first-class object the tooling can reason about:

**`:cost agent_name`** in the REPL renders a cost tree showing which calls contribute what. Not "this agent costs $X" — a hierarchical breakdown of where money goes.

**`corvid eval --swap-model=opus trace.jsonl`** replays a recorded execution against a different model and reports accuracy + cost deltas. Model migration decisions become statistically grounded.

**`corvid routing-report`** analyzes eval history and flags routing rules that are underperforming. The language tells you when your model choices are wrong.

**`:whatif tool returns <json>`** in the REPL replays the last execution with a counterfactual tool result. Safety exploration becomes interactive.

## Why this is new

Every prior effect system (Koka, Eff, Frank, Haskell monad transformers, Unison abilities, Rust `unsafe`) treats effects as flat names or monadic contexts. They all ask "does this function perform IO?" or "does this function raise exceptions?" — never "does this composition exceed a cost budget," "is this answer provably grounded," "can this decision be made autonomously at this confidence level."

Corvid's dimensional effect system is a different kind of object. Effects carry quantitative or categorical dimensions. The composition algebra is per-dimension. Constraints are typed annotations that the compiler proves against the composed profile. The same system handles five distinct safety concerns with one coherent abstraction.

This is what makes Corvid's effect system a genuine language-level moat rather than a library-level convention.

## Related work cross-references

- Koka row polymorphism: flat effect rows, no dimensions. Corvid adds quantitative composition. See [11-related-work.md](./11-related-work.md).
- Rust `unsafe`: binary effect tag. No composition, no dimensions. Corvid generalizes.
- Haskell monad transformers: effects are types but without quantitative composition rules. `cost + cost = cost` is not expressible.
- Capability-based security: closest conceptual neighbor — capabilities are typed permissions. Corvid adds compositional dimensions, not just discrete capabilities.

## Spec structure

Section 01 defines the syntax. Section 02 defines the composition algebra. Section 03 formalizes the typing rules. Sections 04–09 work through each concrete dimension and its associated features. Section 10 covers interactions with FFI, generics, and async. Section 11 compares against prior art. Section 12 documents how we verify the system is sound.

Every runnable example in this spec is compiled against the current Corvid toolchain during publication. If an example breaks, the spec fails CI. The spec cannot lie about the language.
