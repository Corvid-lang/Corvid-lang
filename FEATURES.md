# Corvid — Features & Roadmap

> The feature roadmap from v0.1 through v1.0. Every feature on this list has earned its place. Adding a feature requires justification in `dev-log.md`; removing one requires archiving the rationale.

---

## Guiding rule

For every feature, ask: **"If I remove this, does the language still have a reason to exist?"**

- Remove any v0.1 feature → the pitch dies.
- Remove any v0.2 feature → the language is unusable in real projects.
- Remove any v0.3 feature → the moat disappears.
- v0.4+ features are nice-to-have; they must not delay earlier releases.

---

## v0.1 — Foundation (target: 4–6 months)

The minimum that makes Corvid real. Effect types are the killer demo.

### Language features

1. **Typed prompts as first-class declarations**
   ```
   prompt classify(t: Ticket) -> Category {
     "Classify this ticket into one category."
   }
   ```
   The compiler generates the JSON schema, handles the LLM call, parses the output. No `ai.decide(ReturnType, ...)` — it's syntax.

2. **Tools with `dangerous` annotation**
   ```
   tool get_order(id: String) -> Order
   tool issue_refund(id: String, amt: Float) -> Receipt dangerous
   ```
   Two effect classes: safe (the default) and `dangerous`. Additional classes may arrive later (v0.2+).

3. **Compiler-enforced approval for dangerous effects**
   ```
   # does not compile
   r = issue_refund("ord_42", 500.0)

   # compiles
   approve IssueRefund("ord_42", 500.0)
   r = issue_refund("ord_42", 500.0)
   ```
   **This is the v0.1 demo.** The language refuses to compile unsafe agent code.

4. **Agents as top-level declarations**
   ```
   agent refund_bot(ticket: Ticket) -> Decision {
     let order = get_order(ticket.order_id)
     let d = decide_refund(ticket, order)
     if d.refund {
       approve(IssueRefund(order.id, order.amount))
       issue_refund(order.id, order.amount)
     }
     return d
   }
   ```

5. **Structured output with typed returns**
   Return types are Pydantic-shaped structs. The compiler emits JSON schemas. The runtime validates.

### Infrastructure

6. **Python interop**
   ```
   import python "anthropic" as anthropic
   import python "pandas" as pd
   ```
   Users get the entire Python ecosystem from day one.

7. **CLI runner**
   ```
   corvid new my_project
   corvid check
   corvid build
   corvid run src/refund_bot.cor
   ```

### Polish (non-negotiable)

- One-command install (`curl | sh`).
- World-class compiler error messages (`ariadne`) with fix-it hints.
- A 5-minute tutorial that works flawlessly.
- Side-by-side Python vs Corvid landing page demo.

---

## v0.2 — Retention (target: 8–10 months)

Features that turn triers into users.

1. **Testing primitives** — `mock`, `fixture`, `assert_behavior` as language features.
   ```
   test refund_bot_flags_large_refunds {
     mock decide_refund returns RefundDecision(refund: true, amount: 500)
     let r = refund_bot(fake_ticket())
     assert r.needs_approval
   }
   ```
2. **Multi-provider LLM abstraction** — OpenAI, Google, local models via one interface.
3. **Memory primitives** — `session`, `memory` as typed, SQLite-backed stores.
4. **Error handling** — typed `Result` / `Option`; retry policies as syntax.
5. **Human-in-the-loop beyond `approve`** — `ask(...)` for clarifications, `choose(...)` for options.

---

## v0.3 — Differentiation (target: 12–15 months)

The moat features — hard to copy without a compiler.

1. **Uncertainty types** — `T?confidence`; compiler forces low-confidence handling.
   ```
   let category: Category?confidence = classify(ticket)
   if confidence(category) < 0.8 {
     return escalate(ticket)
   }
   ```
2. **Cost budgets** — compile-time-checked spend caps.
   ```
   agent triage(t: Ticket) -> Action @budget($0.10) { ... }
   ```
3. **Streaming as first-class type** — `Stream<Token>`, `Stream<T>` for partial structured outputs.
4. **Prompt-aware compilation** — compiler deduplicates prompts, caches schemas, emits TOON-compressed payloads.
5. **Replay as a language concept** — every run replayable by construction; `corvid replay` as a primitive.

---

## v0.4 — Scale (target: 18–24 months)

Features for serious production use.

1. **Multi-agent composition** — agents calling agents with automatic trace merging.
2. **Durable execution** — crash-safe by default; no Temporal needed.
3. **Observability built in** — tracing, cost analytics, per-agent dashboards in `target/trace/`.
4. **Policy system** — declarative rate limits, auth, auditing.
5. **Hot reload** — edit agent; in-flight runs keep version; new runs use new code.

---

## v0.5 — Ecosystem (target: year 2, Q3–Q4)

What makes a language a movement.

1. **Package manager** — `corvid add <package>`. Study Cargo; copy what works.
2. **IDE support** — LSP server, VS Code extension, syntax highlighting, inline trace viewer.
3. **Standard library** — common agent patterns (RAG, tool-use, planning) as stdlib.
4. **Eval framework as language feature**
   ```
   eval refund_bot_quality against dataset("./traces/*") {
     assert average_cost < $0.05
     assert approval_rate_on_blockers > 0.95
   }
   ```

---

## v1.0 — Launch (target: year 3)

Native runtime, stable API, production-ready.

1. **Native runtime via Cranelift** — stop transpiling to Python for production code.
2. **WASM target** — browser, Node, Deno, edge.
3. **Stable language spec** — semver `1.0` guarantees, no breaking changes without major bump.
4. **Full docs + tutorial + book**.
5. **Public launch** — HN, conferences, keynote demo.

---

## Explicitly deferred past v1.0

Not necessarily bad ideas, but not v1.0 scope:

- Macros / metaprogramming
- Custom effect definitions (beyond pure/compensable/irreversible)
- Dependent types
- Linear / affine types
- Formal verification hooks
- Distributed agent orchestration
- Multi-model ensemble primitives
- Fine-tuning as a language feature
- Visual/block-based editor

---

## Feature request protocol

To add a feature to this roadmap:

1. Open a section in `dev-log.md` titled `feature-proposal: <name>`.
2. Answer three questions:
   - What pain does this solve that current features don't?
   - What's the smallest version that provides value?
   - What milestone does it belong in?
3. If accepted, add to the appropriate version above with a link to the dev-log entry.

Default answer to feature requests is **no**. Scope discipline is the single most important factor in whether Corvid ships.
