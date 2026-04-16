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

### `List<T>` — ordered collections

Declare with a bracketed literal; index with `[i]`; iterate with `for`:

```corvid
agent sum() -> Int:
    total = 0
    for x in [1, 2, 3, 4, 5]:
        total = total + x
    return total      # 15

agent third_item() -> Int:
    xs = [10, 20, 30]
    return xs[1]      # 20

agent matches(needle: String, haystack: List[String]) -> Bool:
    for s in haystack:
        if s == needle:
            return true
    return false
```

Lists are **immutable** — no `.push` / `.append` / `list[i] = v`. Build a new list instead.

Memory: each list is one heap allocation laid out as `[header | length | element_0 | element_1 | ...]` with 8 bytes per element. Lists with refcounted elements (Strings, Structs, nested Lists) use a shared runtime destructor that walks the length and releases each element — the destructor doesn't need to know the element type because at the runtime level every refcounted element is an I64 pointer. Nested cleanup cascades naturally through each element's own header chain.

**Bounds checking is enforced at runtime.** `xs[5]` on a 3-element list traps with the same error path as integer overflow — exits non-zero with a stderr message. No silent out-of-range reads.

List semantics landed in [dev-log Day 26](dev-log.md).

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

Iterate over a list with `for x in list:`. Escape with `break`; skip to the next iteration with `continue`:

```corvid
agent first_even(xs: List[Int]) -> Int:
    for x in xs:
        if x % 2 == 0:
            return x
        continue         # explicit — `continue` is also fine
    return 0             # no even number found

agent sum_until_negative(xs: List[Int]) -> Int:
    total = 0
    for x in xs:
        if x < 0:
            break
        total = total + x
    return total
```

`break` and `continue` respect the nearest enclosing loop. Loop variables (`x` above) are typed to the list's element type automatically — no need to write `x: Int`.

Loop control landed in [dev-log Day 26](dev-log.md).

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

### `for c in string` not yet in native code

Compiles via the interpreter; raises `NotSupported` in the native compiler. The fix is either a shared iterator protocol or a String-specific lowering path — neither is on the immediate roadmap. Use `for x in list` when you're writing native-targeted code.

### Writing tools in Rust

Phase 14 ships a typed C-ABI for tool dispatch. Users write tool implementations in a Rust crate, decorate them with `#[tool("name")]`, build the crate as a staticlib, and pass the resulting `.lib` / `.a` to `corvid run --with-tools-lib <path>` or `corvid build --target=native --with-tools-lib <path>`.

Example tool crate:

```toml
# Cargo.toml
[package]
name = "my_tools"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["staticlib"]

[dependencies]
corvid-runtime = { path = "../path/to/corvid/crates/corvid-runtime" }
corvid-macros  = { path = "../path/to/corvid/crates/corvid-macros" }
tokio          = { version = "1", features = ["full"] }
```

```rust
// src/lib.rs
use corvid_macros::tool;

#[tool("get_order")]
async fn get_order(id: String) -> String {
    // call your DB, an HTTP API, anything
    format!("order: {id}")
}

#[tool("issue_refund")]
async fn issue_refund(order_id: String, amount: f64) -> i64 {
    // returns the refund ID
    42
}
```

Build + run:

```bash
cd my_tools
cargo build --release    # produces target/release/libmy_tools.a (or .lib on Windows)

cd ../my_corvid_app
corvid run main.cor --with-tools-lib ../my_tools/target/release/libmy_tools.a
```

Phase 14 supported tool signatures (scalars only; Struct/List defer to Phase 15):

| Corvid type | Rust type   |
|-------------|-------------|
| `Int`       | `i64`       |
| `Bool`      | `bool`      |
| `Float`     | `f64`       |
| `String`    | `String`    |

Tools must be `async fn`. Wrap a sync body in `async { ... }` if you don't need to await anything. The tool function name in `#[tool("...")]` matches the Corvid `tool` declaration's name.

Without `--with-tools-lib`, programs that call user tools fall back to the interpreter (auto) or error out (`--target=native`). The interpreter tier needs tool implementations registered separately via `Runtime::builder().tool(...)` in a runner binary — that pattern is unchanged.

### Methods on types (`extend T:` blocks)

Phase 16 attaches methods to user-declared types via `extend T:` blocks. Methods can be ANY declaration kind — agent, prompt, or tool — and all dispatch through the same dot-syntax. The receiver is an explicit first parameter (no `self` keyword); the typechecker and IR rewrite `value.method(args)` into a regular call with the receiver prepended.

```corvid
type Order:
    amount: Int
    tax: Int

extend Order:
    public agent total(o: Order) -> Int:
        return o.amount + o.tax

    public prompt summarize(o: Order) -> String:
        "Summarize this order: amount {o.amount}, tax {o.tax}"

    public tool fetch_status(o: Order) -> Status dangerous

    agent compute_internal(o: Order) -> Int:    # private (default)
        return o.amount / 10
```

Call sites:

```corvid
agent process(o: Order) -> Int:
    t = o.total()              # pure agent call
    pitch = o.summarize()      # LLM dispatch through Phase 15 bridge
    s = o.fetch_status()       # tool dispatch through Phase 14 bridge
    return t
```

Visibility:
- Default is **private** — callable only from code in the same file.
- `public` makes the method callable from anywhere the type is visible.
- `public(package)` reserves package-scoped visibility for Phase 25's package-manager work; syntactically accepted now so user code doesn't need re-annotation later.
- `public(effect: ...)` is the syntactic slot reserved for Phase 20's effect-scoped visibility.

Method-name rules:
- Two methods with the same name on the same type → compile error.
- A method whose name collides with a field on the same type → compile error.
- Methods with the same name on different types coexist (`Order.total`, `Line.total`).
- Methods on built-in types (Int, String, List) defer to a future phase to avoid orphan-rule complexity.

### Performance — when native wins

Phase 12 closed with published numbers (ARCHITECTURE.md §18). End-to-end wall-clock on three representative workloads:

| Workload | Interpreter | Native | Ratio |
|---|---|---|---|
| 500k Int arithmetic ops | 256 ms | 19 ms | 13.6× |
| 50k String concatenations | 48 ms | 18 ms | 2.7× |
| 100k struct alloc + field reads | 74 ms | 21 ms | 3.5× |

**Spawn-tax crossover:** on Windows, every `corvid run` in native mode pays ~11 ms of OS-level process-spawn cost. For programs whose interpreter run-time is under ~5 ms, that tax outweighs the codegen speedup and interpreter wins end-to-end. Above ~20 ms of interpreter compute, native wins decisively. In between, measure.

Auto dispatch (`corvid run` default) still picks native for tool-free programs because the compile cache makes re-runs near-instant and real agent workloads exceed the crossover. Override with `--target=interpreter` for tiny programs where the spawn tax matters.

Reproduce locally: `cargo bench -p corvid-codegen-cl --bench phase12_benchmarks`.

### Running Corvid code

`corvid run <file>` picks the right execution tier automatically:

- **Native AOT** when the program uses only native-able features (arithmetic, Bool, Float, String, struct, list, agent-to-agent calls). First run compiles and caches; subsequent runs of the same source skip codegen entirely (≈15× faster). Cache lives at `<project>/target/cache/native/<hash>[.exe]` and is swept by `cargo clean`.
- **Interpreter** when the program uses anything that needs the async runtime (tool calls, prompt calls, `approve`, `import python`). Auto-fallback announces itself with one stderr line naming the specific construct and the phase that will lift the restriction:

```
↻ running via interpreter: program calls prompt `greet` — native prompt dispatch lands in Phase 14
```

Explicit overrides:

```bash
corvid run foo.cor                         # auto (default)
corvid run foo.cor --target=native         # require native; error if not possible
corvid run foo.cor --target=interpreter    # force interpreter, even when native works
```

Use `--target=native` when you want to catch a regression the moment a change introduces an un-native-able feature. Use `--target=interpreter` when you need traces, the mock LLM runtime, or tool handlers that the native tier can't load yet.

### Entry agent constraints

Native compile accepts **scalar** entry agents: parameters and return may each be `Int`, `Bool`, `Float`, or `String`. `Struct` and `List` at the entry boundary still raise `NotSupported` — they need a serialization slice before they can round-trip through argv / stdout meaningfully. Wrap a composite-taking agent in a thin `main` that parses a `String`:

```corvid
# Native-compile-friendly — argv[1] becomes `name`.
agent greet(name: String) -> String:
    return "hi " + name

# Multi-arg entry — argv[1..3] become a, b.
agent sum(a: Int, b: Int) -> Int:
    return a + b
```

Invoking the binary:

```bash
corvid build greet.cor --target=native
./target/bin/greet world            # prints: hi world
./target/bin/sum 10 32              # prints: 42
```

Format rules the codegen-emitted `main` uses:
- `Bool` on the command line is `true` / `false` (case-sensitive). Result printing matches.
- `Float` is decoded with libc `strtod` and printed with `%.17g` (round-trippable).
- `Int` is decoded with `strtoll`; overflow or non-numeric input exits non-zero with a slice-specific error (not the overflow handler).
- `String` is taken verbatim from argv (UTF-8 pass-through — shells handle quoting).
- Arity mismatch (wrong number of argv args) exits non-zero with a clear message before the agent runs.

### No multi-threading

Corvid is single-threaded today. Atomic refcount is cheap insurance for Phase 25 (multi-agent coordinators).

---

## What's on the near-term roadmap

Per [ROADMAP.md](ROADMAP.md):

- **Phase 17** (next) — Cycle collector on top of refcount. Backstops the refcount runtime against reference cycles using a stop-the-world mark-sweep collector triggered by allocation pressure. Closes the "deterministic destructors leak on cycles" hole without giving up Phase 12g/h's prompt-release property.
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
| List + `for` + `break` / `continue` | [Day 26](dev-log.md) |
| Parameterised entry agents + Float/String entry returns | [Day 27](dev-log.md) |
| Native as the default tier for tool-free programs + compile cache | [Day 28](dev-log.md) |
| Phase 12 close-out benchmarks: native is 2.7×–13.6× faster end-to-end | [Day 29](dev-log.md) |
| Phase 13: tokio + corvid runtime embedded in compiled binaries; native tool dispatch (narrow case) | [Day 30](dev-log.md) |
| Phase 14: `#[tool]` proc-macro + typed C ABI dispatch + `--with-tools-lib` | [Day 31](dev-log.md) |
| Phase 15: native prompt dispatch + 5 LLM provider adapters (Anthropic / OpenAI / OpenAI-compat / Ollama / Gemini) | [Day 32](dev-log.md) |
| Phase 16: methods on types (`extend T:` blocks, mixed agent/prompt/tool, public visibility) | [Day 33](dev-log.md) |
| Slice 17a: typed heap headers + per-type typeinfo + non-atomic refcount | [Day 16](dev-log.md) |
| Slice 17c: Cranelift safepoints + emitted stack-map table | [Day 25](dev-log.md) |
| Slice 17d: cycle collector — mark-sweep over the refcount heap | [Day 26](dev-log.md) |
| Slice 17f++: replay-deterministic GC trigger log + shadow-count refcount verifier with PC blame | [Day 27](dev-log.md) |

---

## Slice 17a — typed heap headers (what it means for users)

**Nothing to change in your Corvid code.** Slice 17a is infrastructure for the upcoming cycle collector (Phase 17d) and the effect-typed memory model (slice 17b). It's behavior-preserving end-to-end — all 105 codegen parity tests pass unchanged.

What changed under the hood:

- **Every refcounted allocation now carries a per-type metadata pointer** (`corvid_typeinfo`) in its 16-byte header. The collector (17d) and the dump/debug tooling (later) both dispatch through this block rather than hardcoding per-type knowledge in the runtime.
- **Refcount is no longer atomic.** Corvid is single-threaded, so the atomic ops were paying a per-retain/release cost (~10-50× vs non-atomic on x86) for a multi-threaded scenario that doesn't exist yet. Phase 25 multi-agent will bring a proper multi-threaded RC design — biased RC or deferred RC, not blanket atomics.
- **`List<Int>`-style primitive lists no longer mis-trace.** The old design couldn't tell at trace time whether a list held pointers or integers; the new typeinfo's `elem_typeinfo = NULL` sentinel is explicit. Compiled programs with `List<Int>` now carry a typeinfo that says "don't chase these slots."
- **Refcount bit-packing.** Top bits of the refcount word are reserved for the cycle collector's mark/color state (17d, 17h). Retain/release preserve those bits under an externally-set mark — pinned by a new runtime test.

What becomes possible next:

- Slice 17b (renamed from "per-task arena"): the effect-typed memory model. Most allocations bump-allocate in a per-scope arena driven by static escape analysis; the compiler elides RC ops entirely on provably-unique values (Perceus-style); in-place reuse converts functional-style updates into bump-free mutations.
- Slice 17d: the cycle collector dispatches through each object's typeinfo during the mark phase. No per-type switch in the collector.
- Slice 17g: `Weak<T>` slots in the typeinfo's reserved `weak_fn` field.

## Slice 17d — cycle collector (what it means for users)

**Nothing to change in your Corvid code.** Slice 17d closes Phase 17's correctness promise: refcount handles the acyclic case in the fast path; a stop-the-world mark-sweep collector reclaims unreachable cycles.

What changed under the hood:

- **Hidden tracking-node prefix** before every refcounted allocation. The user-visible 16-byte header (refcount + typeinfo) is unchanged; the runtime now allocates a 24-byte prefix in front of it that links every live block into a global doubly-linked list. Static-literal codegen is untouched — the prefix is invisible to anything that reads through the public `corvid_alloc_typed` interface.
- **Mark phase walks the RBP chain.** Cranelift's `preserve_frame_pointers` flag is now on, so every Corvid-compiled frame has a standard `[rbp+0]=prev_rbp, rbp+8=return_pc` layout. The collector chases that chain, looks up each return PC in `corvid_stack_maps` (emitted by 17c), and marks every refcounted pointer at the recorded SP-relative offsets.
- **Two-pass sweep.** Pass 1 traces every unmarked block's children with a decrement-only marker so refcount bookkeeping stays consistent for any reachable children that an unreachable block referenced. Pass 2 frees the unmarked blocks and clears mark bits on survivors. The split avoids `destroy_fn` recursion during collection.
- **Allocation-pressure trigger.** `corvid_alloc_typed` fires the collector every `CORVID_GC_TRIGGER` allocations (default 10_000, set via env var; `0` disables auto-GC). Tests use `corvid_gc_from_roots` for deterministic, stack-walk-free invocation.

How to interact with it:

- `CORVID_GC_TRIGGER=N` — fire automatic GC every N allocations. Set to `0` to disable.
- `CORVID_DEBUG_ALLOC=1` — print alloc/release counters at exit (existing knob, still works).

## Slice 17f++ — refcount verifier + GC trigger log (what it means for users)

This is Corvid-specific infrastructure that turns the cycle collector into **a runtime checker for the ownership optimizer (17b)**. Every GC cycle, the verifier traverses the reachable graph, computes the expected refcount of each block from its incoming edges, and diffs against the actual refcount. Drift means a miscompile.

How to use it:

- `CORVID_GC_VERIFY=warn` — verifier runs each GC cycle, prints a drift report to stderr if anything diverges, execution continues. Recommended for CI.
- `CORVID_GC_VERIFY=abort` — same, but `abort()` on any drift. Recommended for fuzzing / bug-hunting.
- `CORVID_GC_VERIFY=off` (default) — verifier skipped. Zero cost on the fast path.

Drift reports include:

```
CORVID_GC_VERIFY: refcount drift
  block:           0x... typeinfo=<name>
  expected_rc:     <count from reachability>
  actual_rc:       <count from refcount word>
  diagnosis:       under-count (missing retain; UAF risk) | over-count (missing release; leak)
  last_retain_pc:  <PC of most recent corvid_retain on this block>
  last_release_pc: <PC of most recent corvid_release, or 0 if never released>
```

The blame PCs are stamped by `corvid_retain` / `corvid_release` via compiler return-address intrinsics — they cost a single store on the already-dirty cache line, no observable overhead in the fast path.

The slice also lays the foundation for **replay-deterministic GC**:

- Every GC cycle appends a record to a trigger log: `(alloc_count, safepoint_count, cycle_index)`.
- A new `corvid_safepoint_count` global plus `corvid_safepoint_notify()` C entry are exposed for codegen / latency-aware triggers (17b-7) to drive collection at compiler-invariant points.
- Phase 19 replay infrastructure can read the log via `corvid_gc_trigger_log_length` / `corvid_gc_trigger_log_at` accessors and replay GC at the same logical points across runs, even if the optimizer changes allocation patterns.

What this gets Corvid:

1. The ownership optimizer (17b) is runtime-verified on every program you run with `VERIFY=1`. No other refcount language ships this — they don't have the typed-graph traversal infrastructure to do it cheaply.
2. Refcount miscompilations carry source-locating blame instead of presenting as silent corruption later.
3. GC trigger points are explicit data the runtime exposes, not a hidden side-effect of allocation pressure — which is what makes replay-time reproduction possible.

## Phase 19 — REPL and replay (how to use it)

Corvid now has an interactive REPL:

```bash
corvid repl
```

The REPL keeps successful declarations and locals across turns. That means you can declare a type, construct a value later, and inspect fields after that in separate inputs.

Example:

```text
>>> type Point:
...     x: Int
...     y: Int
...
>>> p = Point(1, 2)
>>> p.x
2
```

### Multi-line input

If the first line of a turn ends with `:`, the REPL enters multi-line mode and keeps reading with the `... ` prompt until you submit a blank line.

Use this for:

- `type` declarations
- `extend T:` blocks
- multi-line `if` / `for`
- multi-line `try ... on error retry ...` expressions or statements

### What persists across turns

Successful turns commit.
Failed turns roll back.

That applies to:

- declarations
- top-level locals
- the top-level type environment

So a parse/type/runtime error in turn `N` does not poison the session state from turns `1..N-1`.

### Result / Option / `?` / retry in the REPL

The Phase 18 surfaces work directly in `corvid repl`:

```text
>>> Ok(Some("hi"))
Ok(Some("hi"))
```

```text
>>> try flaky_call() on error retry 3 times backoff linear 250
...
```

The REPL prints expression results with type-aware rendering for:

- `Result`
- `Option`
- `Struct`
- `List`
- `String`

Recursive composite values are guarded:

- repeated structural revisits print as `<cycle>`
- overly deep recursion prints as `<...>`

### History and shell behavior

- `Ctrl-D` exits cleanly
- `Ctrl-C` cancels the current in-flight turn
- history persists across sessions

History file location:

- Unix: `$XDG_DATA_HOME/corvid/history`
- Unix fallback: `~/.local/share/corvid/history`
- Windows: `%APPDATA%\\corvid\\history`

### Replay stepping

The REPL can load an existing JSONL runtime trace and step through it:

```text
>>> :replay target/trace/run-1713199999999.jsonl
loaded replay `target/trace/run-1713199999999.jsonl` [run run-1713199999999]: 5 step(s), 70 ms, final status: OK
```

Replay commands:

- `:step` or `:s` advances one recorded step
- bare `Enter` in replay mode also advances one step
- `:step N` advances `N` steps
- `:run` plays to the end
- `:show` reprints the current step
- `:where` shows the current position
- `:quit` or `:q` leaves replay mode and returns to the normal REPL

Replay output shows the recorded data, not reconstructed guesses:

- run start inputs
- tool call args and recorded results
- LLM prompt name, model, rendered prompt text, args, and recorded result
- approval request args and recorded decision
- final run result or error

If a trace is incomplete, replay reports `TRUNCATED` and still shows the recorded prefix. If the file is malformed or not a valid Corvid trace, the REPL prints a clear error and stays in normal mode.

## Weak References

Phase 17g adds first-class weak references with effect-typed invalidation.

### Basic syntax

`Weak<T>` means "a weak reference to `T`, with runtime checks only."

```corvid
agent cache(name: String) -> Weak<String>:
    return Weak::new(name)
```

`Weak<T, {effects}>` is the powerful form. The effect row says which effects may invalidate the checker’s proof that the weak is still fresh.

```corvid
agent cache(name: String) -> Weak<String, {tool_call, llm}>:
    return Weak::new(name)
```

Supported weak-effect names today:

- `tool_call`
- `llm`
- `approve`

### Construction and upgrade

```corvid
agent load(name: String) -> Option<String>:
    w = Weak::new(name)
    return Weak::upgrade(w)
```

`Weak::new(...)` refreshes the weak at the current effect frontier.

`Weak::upgrade(...)` returns `Option<T>`:

- `Some(value)` if the strong target is still alive
- `None` if the target has been cleared

### What the checker proves

The checker tracks whether an invalidating effect may have happened since the weak was last refreshed.

This is accepted:

```corvid
agent make(name: String) -> Weak<String, {tool_call}>:
    return Weak::new(name)

agent load(name: String) -> Option<String>:
    w = make(name)
    return Weak::upgrade(w)
```

This is rejected:

```corvid
tool fetch_name(id: String) -> String

agent make(name: String) -> Weak<String, {tool_call}>:
    return Weak::new(name)

agent load(name: String) -> Option<String>:
    w = make(name)
    fetch_name(name)
    return Weak::upgrade(w)
```

Why: `tool_call` is in the weak’s effect row, and there was no intervening refresh before the upgrade.

### Refresh rules

- `Weak::new(strong)` refreshes at the current effect frontier.
- successful `Weak::upgrade(w)` refreshes `w` at the current effect frontier.
- at control-flow merges, a weak is considered refreshed only if **all** incoming paths refreshed it.

This merge is therefore rejected:

```corvid
tool fetch_name(id: String) -> String

agent make(name: String) -> Weak<String, {tool_call}>:
    return Weak::new(name)

agent load(flag: Bool, name: String) -> Option<String>:
    w = make(name)
    if flag:
        Weak::upgrade(w)
    else:
        keep = name
    fetch_name(name)
    return Weak::upgrade(w)
```

One path refreshed `w`, one path did not, so after the merge the checker keeps the weaker fact.

### Runtime guarantees

On the native runtime:

- weak slots clear when the strong target’s refcount reaches zero
- weak slots also clear during GC sweep of unreachable cycles
- clearing happens before destroy-time re-entrancy can observe a stale pointer

The direct runtime weak tests live in:

- `crates/corvid-runtime/tests/weak.rs`

Current native parity coverage proves the live-upgrade path. A stronger source-level overwrite/drop parity case is still under audit in the native codegen / ownership interaction.

## VM Heap Handles

Slice 17h.1 moved the interpreter's cycle-capable values onto VM-owned retain/release metadata in preparation for Bacon-Rajan cycle collection.

What changed:

- `Struct`, `List`, and boxed `Result` / `OptionSome` payloads no longer rely only on raw `Arc` clone/drop semantics for their Corvid-level lifetime.
- the interpreter now owns explicit retain/release accounting for those graph nodes
- native and VM heaps are still completely separate implementations; they only need to agree behaviourally

Important boundary:

- `String` stays a leaf `Arc<str>` in 17h.1

Why that boundary is honest:

- strings are heap values, but not cycle-forming graph nodes
- Bacon-Rajan needs ownership over the graph edges, and those live in struct/list/boxed payloads, not in leaf strings

Practical implication:

- this commit is the prerequisite plumbing for VM cycle collection, not the collector itself
- Bacon-Rajan lands on top of these VM-owned graph handles in 17h.2

## Contributing / feedback

See [CONTRIBUTING.md](CONTRIBUTING.md). The rules of the road are: pre-phase chat before code, per-slice commits at every boundary, dev-log entry for every session, no shortcuts. The `learnings.md` file you're reading gets updated when each slice ships a user-visible feature.
