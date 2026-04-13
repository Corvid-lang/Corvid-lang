# Corvid — learnings

> A pragmatic guide to what you can write in Corvid today, how to run it, and where the edges are. Grows with every slice that adds a user-visible feature. Cross-references to the dev-log where decisions were made.

This is the "how to actually use Corvid" document. For the pitch, see [README.md](README.md). For the full feature roadmap, [FEATURES.md](FEATURES.md). For architecture, [ARCHITECTURE.md](ARCHITECTURE.md). For the build journal, [dev-log.md](dev-log.md).

---

## Quick start

```bash
# Build the compiler
cargo install --path crates/corvid-cli

# Write your first program
cat > hello.cor <<'EOF'
agent main() -> Int:
    return 42
EOF

# Compile to a native binary (one .exe, no runtime installer)
corvid build --target=native hello.cor
./target/bin/hello     # prints: 42
```

Corvid is Python-shaped on the surface, with AI-native primitives on top. You already know how to read most of it.

---

## File structure

```
my_project/
├── corvid.toml          # project config (optional — not needed for single files)
├── src/
│   └── main.cor         # convention: put sources under src/
└── target/
    ├── bin/             # native binaries (from --target=native)
    ├── py/              # generated Python (from --target=python)
    └── trace/           # JSONL traces from runtime calls
```

Single `.cor` files compile fine without any project structure. The `corvid build` command creates `target/` alongside wherever the source lives.

---

## Types & values

Corvid has five scalar-and-composite value types shipping in the native compiler today.

### `Int` — 64-bit signed integer

```corvid
agent n() -> Int:
    return 42
```

Arithmetic (`+`, `-`, `*`, `/`, `%`) **traps on overflow**. `i64::MAX + 1` does not silently wrap — the binary exits with a runtime error:

```
corvid: runtime error: integer overflow or division by zero
```

Division and modulo by zero trap the same way. The safety story behind this is in [dev-log Day 19](dev-log.md).

If you want wrapping arithmetic (e.g., hash mixing), a `@wrapping` annotation is on the Phase-22 roadmap.

### `Bool` — true / false

```corvid
agent is_even(n: Int) -> Bool:
    return n % 2 == 0
```

Stored as a single byte internally (`I8`). No truthy/falsy coercion — `if 0:` is a type error, not "it's falsy so skip." Bool is its own type.

`and` / `or` **short-circuit** on both the interpreter and the native binary. The right side is not evaluated if the left determines the answer:

```corvid
# This returns true — the divide-by-zero is skipped.
agent f() -> Bool:
    return true or (1 / 0 == 0)
```

Short-circuit semantics landed in [dev-log Day 20](dev-log.md).

### `Float` — 64-bit IEEE 754

```corvid
agent total(price: Float, quantity: Int) -> Float:
    return price * quantity
```

Follows **IEEE 754 semantics** — no traps:
- `1.0 / 0.0` returns `+Inf`
- `0.0 / 0.0` returns `NaN`
- `NaN == NaN` is `false`; `NaN != NaN` is `true`

Mixed `Int + Float` promotes to `Float` (same widening rule as Python). Why IEEE and not trap-on-divide? Float's design intent is that `Inf`/`NaN` ARE the safety mechanism — they propagate upstream errors without aborting. Design rationale in [dev-log Day 22](dev-log.md).

Int vs Float policy:
- Int traps — integer overflow is never the desired behavior.
- Float follows IEEE — `Inf`/`NaN` carry meaning.

### `String` — immutable UTF-8

```corvid
agent greet(name: String) -> String:
    return "hello, " + name

agent matches(a: String, b: String) -> Bool:
    return a == b
```

Operators: `+` (concat), `==`, `!=`, `<`, `<=`, `>`, `>=` (bytewise lexicographic — matches Unicode codepoint order for the BMP).

String literals live in `.rodata` and are **immortal** — retain/release on them are no-ops. Concatenated strings are heap-allocated and automatically freed when their last reference goes away (refcount reaches zero). No manual memory management.

What's NOT supported yet:
- String length (`len(s)` / `s.len`) — needs a `len` builtin mechanism, Phase 22+
- Indexing (`s[0]`) — Phase 22+
- Iteration (`for c in s`) — needs iterator protocol, future slice
- Slicing / case-folding / search — stdlib phase

String semantics landed in [dev-log Day 24](dev-log.md); the memory model behind them is [dev-log Day 23](dev-log.md).

### `Struct` — user-declared records

Declare with `type`, construct with `TypeName(args)`, access fields with `.`:

```corvid
type Order:
    id: String
    amount: Float

type Ticket:
    message: String
    refund: Order

agent is_expensive(o: Order) -> Bool:
    return o.amount > 100.0

agent main() -> Bool:
    t = Ticket("damaged", Order("ord_42", 49.99))
    return t.refund.amount > 10.0
```

Field access can nest arbitrarily (`t.refund.amount`). Structs are **immutable** — there's no `s.foo = x` assignment; build a new struct instead.

Memory: each struct is a single heap allocation laid out as `[header | field0 | field1 | ...]` with 8 bytes per field. Structs with refcounted fields (Strings, nested Structs) get an auto-generated destructor that releases each refcounted field before the struct itself is freed. No leaks possible at the language level — the leak detector verifies this on every parity test.

Struct semantics + constructor syntax landed in [dev-log Day 25](dev-log.md).

---

## Local bindings

Python-style bare assignment. **No `let` keyword.**

```corvid
agent calc() -> Int:
    x = 10
    y = 20
    return x + y
```

Reassignment reuses the same binding:

```corvid
agent run() -> Int:
    total = 0
    total = total + 5
    total = total * 2
    return total     # 10
```

Type of a binding is inferred from the initial value. You can't change a binding's type via reassignment (the type checker enforces this).

### Scoping

Bindings introduced inside `if` / `else` branches are **not visible after** the branch — they belong to that branch's scope:

```corvid
agent run(flag: Bool) -> Int:
    if flag:
        x = 1
    # x is not accessible here — return would error.
    return 0
```

If you want a binding visible after an `if`, declare it before:

```corvid
agent run(flag: Bool) -> Int:
    x = 0
    if flag:
        x = 1
    return x
```

Local binding semantics landed in [dev-log Day 21](dev-log.md).

---

## Control flow

### `if` / `else`

Statement-only (not an expression). Both branches can return or fall through:

```corvid
agent classify(n: Int) -> Int:
    if n > 0:
        return 1
    else:
        if n < 0:
            return -1
        else:
            return 0
```

### `pass`

No-op statement. Useful as a placeholder in an empty branch:

```corvid
agent noop_check(x: Int) -> Int:
    if x > 0:
        pass     # we know x is positive; nothing to do here
    return x
```

### `for` / `break` / `continue`

Not yet in the native compiler. Planned for slice 12h. Works in the interpreter today (`corvid run <file>` uses the interpreter).

---

## Agents

Every Corvid program is a collection of `agent` declarations. Agents have typed parameters and a typed return, and they can call each other:

```corvid
agent double(n: Int) -> Int:
    return n * 2

agent quadruple(n: Int) -> Int:
    return double(double(n))

agent main() -> Int:
    return quadruple(5)   # 20
```

Recursion works. Mutual recursion works. The `main` agent (when one exists) is the entry point for `corvid build --target=native`; if there's only one agent, it's the entry by default.

### Entry agent restrictions (native compile only)

`corvid build --target=native` currently requires the entry agent to:

- Take **no parameters** — argv decoding lands in slice 12i.
- Return `Int` or `Bool` — Float / String / Struct returns land in slice 12i (the C shim needs a print-format variant).

Non-entry agents have none of these restrictions. A program can compose any types internally; only the outermost entry is constrained.

Interpreter path (`corvid run`) has no such restrictions — use it when you need to drive real runtime calls (tools, prompts, approvals).

---

## Tools, prompts, approvals — AI-native primitives

Corvid's AI surface uses three keywords: `tool`, `prompt`, `approve`.

### `tool` — external operations

```corvid
tool get_order(id: String) -> Order
tool issue_refund(id: String, amount: Float) -> Receipt dangerous
```

Tools have typed parameters and returns, no body — they're implemented in the host (Python or Rust runtime). The `dangerous` keyword marks tools that can't run without a prior `approve`.

### `prompt` — typed LLM calls

```corvid
prompt decide_refund(ticket: Ticket, order: Order) -> Decision:
    """
    Decide whether this ticket deserves a refund.
    Consider the order amount, the user's complaint, and fairness.
    """
```

Prompts route through a registered LLM adapter (Anthropic, OpenAI) and return a typed struct. The model is instructed to emit structured output matching the return type's schema — no string parsing at the caller.

### `approve` — compile-time safety

```corvid
agent refund_bot(ticket: Ticket) -> Decision:
    order = get_order(ticket.order_id)
    decision = decide_refund(ticket, order)

    if decision.should_refund:
        approve IssueRefund(order.id, order.amount)   # <-- required
        issue_refund(order.id, order.amount)           # <-- dangerous tool call

    return decision
```

Remove the `approve` line and the program **will not compile**:

```
[E0101] error: dangerous tool `issue_refund` called without a prior `approve`
```

This is the killer feature. Enforced at compile time, not runtime. Works in both `corvid run` (interpreter) and `corvid build --target=python`.

### Current compilation gap

`corvid build --target=native` doesn't yet wire tool / prompt / approve calls into compiled code — Phase 14 adds native tool dispatch via a proc-macro `#[tool]` registry. For now, AI-shaped programs run via:

```bash
corvid run refund_bot.cor                     # interpreter, fully native runtime
corvid build --target=python refund_bot.cor   # generates .py you can run with the Python runtime
```

The interpreter path has the full AI runtime (Anthropic + OpenAI adapters, approval flow, tracing, `.env` loading, secret redaction). See the demo: `cargo run -p refund_bot_demo`.

---

## Compilation targets

### `--target=native` (default when slice 12j lands)

```bash
corvid build --target=native src/program.cor
# → target/bin/program[.exe]
```

One binary, no runtime installer, statically linked. Uses Cranelift for codegen; the system C toolchain for linking (`cl.exe` on Windows, `cc` elsewhere).

Supports: Int / Bool / Float / String / Struct + all operators + `if`-`else` + local bindings + agent-to-agent calls.

### `--target=python`

```bash
corvid build --target=python src/program.cor
# → target/py/program.py
```

Generates runnable Python. The `corvid-runtime` Python package provides `tool_call` / `approve_gate` / `llm_call`. Useful when you want to deploy into an existing Python environment or stack.

### `corvid run` (interpreter)

```bash
corvid run src/program.cor
```

Executes via the Rust tree-walking interpreter in `corvid-vm`. Full AI runtime available (tools, prompts, approvals, tracing). Use this for day-to-day development and for AI-shaped programs until Phase 14.

---

## Error model

### Compile-time errors

- **Type errors** (wrong operand types, missing fields, etc.) — reported with line + column + fix hint via ariadne rendering.
- **Effect errors** (`E0101` — dangerous tool called without approve) — the headline safety check.
- **Resolution errors** (undefined names, duplicate declarations).

Every error has a code (`E0001`–`E0302`) and a suggested fix. See [ARCHITECTURE.md §8](ARCHITECTURE.md#L336) for the design target.

### Runtime errors (native binaries)

A compiled program can fail at runtime for:
- **Integer overflow / division by zero** — prints to stderr, exits 1.

That's currently the full list. Approval denial, tool failures, LLM failures only apply to the interpreter/Python paths (Phase 14 adds them to native).

### Runtime errors (interpreter)

- Arithmetic (overflow, div-zero)
- Type mismatch (belt-and-braces for typechecker bypass)
- Index out of bounds
- Missing field on struct
- Tool dispatch failed / unknown tool
- Approval denied
- LLM adapter failed / no model configured

All carry a source span so the error points at the offending code.

---

## Memory model

**Refcounted heap with automatic cleanup.** You never write `free` and you never see a leak.

- Every non-scalar value (String, Struct, eventually List) lives behind a 16-byte header: `atomic refcount + reserved`.
- Static literals have refcount = `i64::MIN` (immortal) — retain/release on them are no-ops.
- Structs with refcounted fields (Strings etc.) get an auto-generated destructor that releases each refcounted field when the struct is freed.
- Refcount updates are atomic — single-threaded today, but Phase 25 multi-agent won't need a migration.

### Leak verification

Every parity test runs the compiled binary with `CORVID_DEBUG_ALLOC=1`:

```bash
CORVID_DEBUG_ALLOC=1 ./target/bin/program
# → program output on stdout
# → stderr: ALLOCS=3\nRELEASES=3
```

The test suite asserts `ALLOCS == RELEASES` on every fixture. Any codegen bug that drops a release would fail the test immediately with the exact delta. As of dev-log Day 25, all 66 parity fixtures pass the leak check.

### When it matters for you

For short-lived programs (agents that run once and exit), refcount overhead is invisible. For long-running services (future RAG servers, Phase 25 multi-agent coordinators), the leak-free guarantee means a Corvid service can run for days/weeks without memory growth. Memory-management design rationale: [dev-log Day 23](dev-log.md) (foundation) and [dev-log Day 24](dev-log.md) (ownership wiring).

---

## Gotchas

### No `let` keyword

Python-style bare assignment. `let x = 5` is a parse error.

```corvid
# Wrong
let x = 5

# Right
x = 5
```

### No `if` as expression

`if` is a statement, not an expression. `x = if cond: 1 else: 2` doesn't parse. Use either a separate `if`/`else` writing to a pre-declared variable, or call helper agents.

### No string interpolation (yet)

`f"hello {name}"` doesn't exist. Use `"hello " + name`. String templating inside `prompt` bodies works differently — see the refund_bot demo.

### No `len` / indexing on strings yet

`s.len` and `s[0]` aren't supported yet. Planned for Phase 22+.

### No `for` in the native compiler yet

Works in the interpreter; compiled path raises `NotSupported` pointing at slice 12h (next slice).

### Entry agent constraints

Native compile requires the entry agent to take no parameters and return Int or Bool. Wrap a parameterised agent in a thin `main` until slice 12i lifts this:

```corvid
agent process(input: Int) -> Int:
    return input * 2

# For native compile, add a parameter-less main:
agent main() -> Int:
    return process(21)
```

### No multi-threading

Corvid is single-threaded today. Atomic refcount is cheap insurance for Phase 25 (multi-agent coordinators).

---

## What's on the near-term roadmap

Per [ROADMAP.md](ROADMAP.md):

- **Slice 12h** (next) — `List<T>`, `for x in list`, `break`, `continue`. Follows the same memory-management pattern as Struct.
- **Slice 12i** — parameterised entry agents (argv decoding), non-Int/Bool entry returns (shim print-format dispatch).
- **Slice 12j** — make native the default for tool-free programs; `corvid run` AOT-compiles and executes when possible.
- **Slice 12k** — polish, benchmarks, stability guarantees.
- **Phase 14** — proc-macro `#[tool]` registry, tool/prompt/approve in compiled code.
- **Phase 16** — effect-tagged `import python "..."` (TypeScript `.d.ts` analog).
- **Phase 18** — Google + Ollama LLM adapters alongside the current Anthropic + OpenAI.
- **Phase 20** — typed `Result` + retry policies.
- **Phase 22** — streaming (`Stream<T>`), cost budgets (`@budget($)`), uncertainty types (`T?confidence`), replay as a language primitive, `@wrapping` opt-out for Int arithmetic, `@checked` for Float.
- **Phase 25** — multi-agent composition + durable execution.

Features earn their slice through real pull, not speculation. Adding something to the roadmap requires a proposal in `dev-log.md` per the rules in [CONTRIBUTING.md](CONTRIBUTING.md).

---

## Per-slice feature log

Each user-visible feature lands with a dev-log entry explaining the design decisions. Cross-references:

| Feature | Dev-log |
|---|---|
| Int arithmetic + overflow trap | [Day 19](dev-log.md) |
| Bool, comparisons, `if`/`else`, short-circuit | [Day 20](dev-log.md) |
| Local bindings + reassignment + `pass` | [Day 21](dev-log.md) |
| Float + IEEE 754 semantics | [Day 22](dev-log.md) |
| Memory management foundation (refcount + leak detector) | [Day 23](dev-log.md) |
| String operations + ownership wiring | [Day 24](dev-log.md) |
| Struct + constructors + destructors | [Day 25](dev-log.md) |

---

## Contributing / feedback

See [CONTRIBUTING.md](CONTRIBUTING.md). The rules of the road are: pre-phase chat before code, per-slice commits at every boundary, dev-log entry for every session, no shortcuts. The `learnings.md` file you're reading gets updated when each slice ships a user-visible feature.
