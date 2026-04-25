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

If you want wrapping arithmetic (e.g., hash mixing), a `@wrapping` annotation is on the long-range roadmap.

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
- String length (`len(s)` / `s.len`) — needs a `len` builtin mechanism, planned future work
- Indexing (`s[0]`) — planned future work
- Iteration (`for c in s`) — needs iterator protocol, future slice
- Slicing / case-folding / search — stdlib work

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

`corvid build --target=native` doesn't yet wire tool / prompt / approve calls into compiled code — native tool dispatch is provided by a proc-macro `#[tool]` registry. For now, AI-shaped programs run via:

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

Executes via the Rust tree-walking interpreter in `corvid-vm`. Full AI runtime available (tools, prompts, approvals, tracing). Use this for day-to-day development and for AI-shaped programs until the native AI runtime path is complete.

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

That's currently the full list. Approval denial, tool failures, and LLM failures only apply to the interpreter/Python paths until the native dispatch path is complete.

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
- Refcount updates are atomic — single-threaded today, but future multi-agent work won't need a migration.

### Leak verification

Every parity test runs the compiled binary with `CORVID_DEBUG_ALLOC=1`:

```bash
CORVID_DEBUG_ALLOC=1 ./target/bin/program
# → program output on stdout
# → stderr: ALLOCS=3\nRELEASES=3
```

The test suite asserts `ALLOCS == RELEASES` on every fixture. Any codegen bug that drops a release would fail the test immediately with the exact delta. As of dev-log Day 25, all 66 parity fixtures pass the leak check.

### When it matters for you

For short-lived programs (agents that run once and exit), refcount overhead is invisible. For long-running services (future RAG servers and multi-agent coordinators), the leak-free guarantee means a Corvid service can run for days/weeks without memory growth. Memory-management design rationale: [dev-log Day 23](dev-log.md) (foundation) and [dev-log Day 24](dev-log.md) (ownership wiring).

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

`s.len` and `s[0]` aren't supported yet. Planned future work.

### `for c in string` not yet in native code

Compiles via the interpreter; raises `NotSupported` in the native compiler. The fix is either a shared iterator protocol or a String-specific lowering path — neither is on the immediate roadmap. Use `for x in list` when you're writing native-targeted code.

### Writing tools in Rust

Native tool dispatch ships with a typed C ABI. Users write tool implementations in a Rust crate, decorate them with `#[tool("name")]`, build the crate as a staticlib, and pass the resulting `.lib` / `.a` to `corvid run --with-tools-lib <path>` or `corvid build --target=native --with-tools-lib <path>`.

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

Currently supported tool signatures (scalars only; Struct/List support comes later):

| Corvid type | Rust type   |
|-------------|-------------|
| `Int`       | `i64`       |
| `Bool`      | `bool`      |
| `Float`     | `f64`       |
| `String`    | `String`    |

Tools must be `async fn`. Wrap a sync body in `async { ... }` if you don't need to await anything. The tool function name in `#[tool("...")]` matches the Corvid `tool` declaration's name.

Without `--with-tools-lib`, programs that call user tools fall back to the interpreter (auto) or error out (`--target=native`). The interpreter tier needs tool implementations registered separately via `Runtime::builder().tool(...)` in a runner binary — that pattern is unchanged.

### Methods on types (`extend T:` blocks)

Methods attach to user-declared types via `extend T:` blocks. Methods can be ANY declaration kind — agent, prompt, or tool — and all dispatch through the same dot-syntax. The receiver is an explicit first parameter (no `self` keyword); the typechecker and IR rewrite `value.method(args)` into a regular call with the receiver prepended.

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
    pitch = o.summarize()      # LLM dispatch through the native prompt bridge
    s = o.fetch_status()       # tool dispatch through the native tool bridge
    return t
```

Visibility:
- Default is **private** — callable only from code in the same file.
- `public` makes the method callable from anywhere the type is visible.
- `public(package)` reserves package-scoped visibility for future package-manager work; syntactically accepted now so user code doesn't need re-annotation later.
- `public(effect: ...)` is the syntactic slot reserved for future effect-scoped visibility.

Method-name rules:
- Two methods with the same name on the same type → compile error.
- A method whose name collides with a field on the same type → compile error.
- Methods with the same name on different types coexist (`Order.total`, `Line.total`).
- Methods on built-in types (Int, String, List) defer to later work to avoid orphan-rule complexity.

### Performance — when native wins

The first native-runtime close shipped with published numbers (ARCHITECTURE.md §18). End-to-end wall-clock on three representative workloads:

| Workload | Interpreter | Native | Ratio |
|---|---|---|---|
| 500k Int arithmetic ops | 256 ms | 19 ms | 13.6× |
| 50k String concatenations | 48 ms | 18 ms | 2.7× |
| 100k struct alloc + field reads | 74 ms | 21 ms | 3.5× |

**Spawn-tax crossover:** on Windows, every `corvid run` in native mode pays ~11 ms of OS-level process-spawn cost. For programs whose interpreter run-time is under ~5 ms, that tax outweighs the codegen speedup and interpreter wins end-to-end. Above ~20 ms of interpreter compute, native wins decisively. In between, measure.

Auto dispatch (`corvid run` default) still picks native for tool-free programs because the compile cache makes re-runs near-instant and real agent workloads exceed the crossover. Override with `--target=interpreter` for tiny programs where the spawn tax matters.

Reproduce locally: `cargo bench -p corvid-codegen-cl --bench native_foundation_benchmarks`.

### Running Corvid code

`corvid run <file>` picks the right execution tier automatically:

- **Native AOT** when the program uses only native-able features (arithmetic, Bool, Float, String, struct, list, agent-to-agent calls). First run compiles and caches; subsequent runs of the same source skip codegen entirely (≈15× faster). Cache lives at `<project>/target/cache/native/<hash>[.exe]` and is swept by `cargo clean`.
- **Interpreter** when the program uses anything that needs the async runtime (tool calls, prompt calls, `approve`, `import python`). Auto-fallback announces itself with one stderr line naming the specific construct and the native feature gap:

```
↻ running via interpreter: program calls prompt `greet` — native prompt dispatch is not available on this path yet
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

Corvid is single-threaded today. Atomic refcount is cheap insurance for future multi-agent coordinators.

---

## What's on the near-term roadmap

Per [ROADMAP.md](ROADMAP.md):

- **Cycle collection on top of refcount** — backstops the refcount runtime against reference cycles using a stop-the-world mark-sweep collector triggered by allocation pressure.
- **Polish, benchmarks, and stability guarantees**
- **Compiled-code tool / prompt / approve support** — proc-macro `#[tool]` registry and native AI dispatch.
- **Effect-tagged `import python "..."`** — TypeScript `.d.ts` analog.
- **More LLM adapters** — Google + Ollama alongside Anthropic + OpenAI.
- **Typed `Result` + retry policies**
- **Streaming, cost budgets, uncertainty types, replay as a language primitive, and arithmetic annotations**
- **Multi-agent composition + durable execution**

Features earn their place through real pull, not speculation. Adding something to the roadmap requires a proposal in `dev-log.md` per the rules in [CONTRIBUTING.md](CONTRIBUTING.md).

---

## Feature Log

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
| Native-runtime close-out benchmarks: native is 2.7×–13.6× faster end-to-end | [Day 29](dev-log.md) |
| Tokio + corvid runtime embedded in compiled binaries; narrow native tool dispatch | [Day 30](dev-log.md) |
| `#[tool]` proc-macro + typed C ABI dispatch + `--with-tools-lib` | [Day 31](dev-log.md) |
| Native prompt dispatch + 5 LLM provider adapters (Anthropic / OpenAI / OpenAI-compat / Ollama / Gemini) | [Day 32](dev-log.md) |
| Methods on types (`extend T:` blocks, mixed agent/prompt/tool, public visibility) | [Day 33](dev-log.md) |
| Typed heap headers + per-type typeinfo + non-atomic refcount | [Day 16](dev-log.md) |
| Cranelift safepoints + emitted stack-map table | [Day 25](dev-log.md) |
| Cycle collector — mark-sweep over the refcount heap | [Day 26](dev-log.md) |
| Replay-deterministic GC trigger log + shadow-count refcount verifier with PC blame | [Day 27](dev-log.md) |

---

## Typed Heap Headers (what it means for users)

**Nothing to change in your Corvid code.** This is infrastructure for the cycle collector and the effect-typed memory model. It's behavior-preserving end-to-end — all 105 codegen parity tests pass unchanged.

What changed under the hood:

- **Every refcounted allocation now carries a per-type metadata pointer** (`corvid_typeinfo`) in its 16-byte header. The collector (17d) and the dump/debug tooling (later) both dispatch through this block rather than hardcoding per-type knowledge in the runtime.
- **Refcount is no longer atomic.** Corvid is single-threaded, so the atomic ops were paying a per-retain/release cost (~10-50× vs non-atomic on x86) for a multi-threaded scenario that doesn't exist yet. Future multi-agent work will bring a proper multi-threaded RC design — biased RC or deferred RC, not blanket atomics.
- **`List<Int>`-style primitive lists no longer mis-trace.** The old design couldn't tell at trace time whether a list held pointers or integers; the new typeinfo's `elem_typeinfo = NULL` sentinel is explicit. Compiled programs with `List<Int>` now carry a typeinfo that says "don't chase these slots."
- **Refcount bit-packing.** Top bits of the refcount word are reserved for the cycle collector's mark/color state (17d, 17h). Retain/release preserve those bits under an externally-set mark — pinned by a new runtime test.

What becomes possible next:

- The effect-typed memory model: most allocations bump-allocate in a per-scope arena driven by static escape analysis; the compiler elides RC ops entirely on provably-unique values (Perceus-style); in-place reuse converts functional-style updates into bump-free mutations.
- The cycle collector dispatches through each object's typeinfo during the mark phase. No per-type switch in the collector.
- `Weak<T>` slots into the typeinfo's reserved `weak_fn` field.

## Cycle Collector (what it means for users)

**Nothing to change in your Corvid code.** This closes the memory-foundation correctness promise: refcount handles the acyclic case in the fast path; a stop-the-world mark-sweep collector reclaims unreachable cycles.

What changed under the hood:

- **Hidden tracking-node prefix** before every refcounted allocation. The user-visible 16-byte header (refcount + typeinfo) is unchanged; the runtime now allocates a 24-byte prefix in front of it that links every live block into a global doubly-linked list. Static-literal codegen is untouched — the prefix is invisible to anything that reads through the public `corvid_alloc_typed` interface.
- **Mark phase walks the RBP chain.** Cranelift's `preserve_frame_pointers` flag is now on, so every Corvid-compiled frame has a standard `[rbp+0]=prev_rbp, rbp+8=return_pc` layout. The collector chases that chain, looks up each return PC in `corvid_stack_maps` (emitted by 17c), and marks every refcounted pointer at the recorded SP-relative offsets.
- **Two-pass sweep.** Pass 1 traces every unmarked block's children with a decrement-only marker so refcount bookkeeping stays consistent for any reachable children that an unreachable block referenced. Pass 2 frees the unmarked blocks and clears mark bits on survivors. The split avoids `destroy_fn` recursion during collection.
- **Allocation-pressure trigger.** `corvid_alloc_typed` fires the collector every `CORVID_GC_TRIGGER` allocations (default 10_000, set via env var; `0` disables auto-GC). Tests use `corvid_gc_from_roots` for deterministic, stack-walk-free invocation.

How to interact with it:

- `CORVID_GC_TRIGGER=N` — fire automatic GC every N allocations. Set to `0` to disable.
- `CORVID_DEBUG_ALLOC=1` — print alloc/release counters at exit (existing knob, still works).

## Refcount Verifier + GC Trigger Log (what it means for users)

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
- Replay infrastructure can read the log via `corvid_gc_trigger_log_length` / `corvid_gc_trigger_log_at` accessors and replay GC at the same logical points across runs, even if the optimizer changes allocation patterns.

What this gets Corvid:

1. The ownership optimizer (17b) is runtime-verified on every program you run with `VERIFY=1`. No other refcount language ships this — they don't have the typed-graph traversal infrastructure to do it cheaply.
2. Refcount miscompilations carry source-locating blame instead of presenting as silent corruption later.
3. GC trigger points are explicit data the runtime exposes, not a hidden side-effect of allocation pressure — which is what makes replay-time reproduction possible.

## REPL And Replay (how to use it)

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

The `Result` / `Option` / retry surfaces work directly in `corvid repl`:

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

Corvid now has first-class weak references with effect-typed invalidation.

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

VM Heap Handles moved the interpreter's cycle-capable values onto VM-owned retain/release metadata in preparation for Bacon-Rajan cycle collection.

What changed:

- `Struct`, `List`, and boxed `Result` / `OptionSome` payloads no longer rely only on raw `Arc` clone/drop semantics for their Corvid-level lifetime.
- the interpreter now owns explicit retain/release accounting for those graph nodes
- native and VM heaps are still completely separate implementations; they only need to agree behaviourally

Important boundary:

- `String` stays a leaf `Arc<str>` in 17h.1
- if a future string-like value ever gains an outgoing refcounted edge
  (for example a rope node or a parent-backed string view), it must move
  onto a VM heap handle and participate in Bacon-Rajan like any other
  graph node

Why that boundary is honest:

- strings are heap values, but not cycle-forming graph nodes
- Bacon-Rajan needs ownership over the graph edges, and those live in struct/list/boxed payloads, not in leaf strings

Practical implication:

- this commit is the prerequisite plumbing for VM cycle collection, not the collector itself
- Bacon-Rajan lands on top of these VM-owned graph handles in 17h.2

## VM Cycle Collection

VM Cycle Collection adds Bacon-Rajan trial deletion to the interpreter tier.

What is collected:

- VM-owned graph nodes: `Struct`, `List`, `ResultOk`, `ResultErr`, and `OptionSome`

What is not collected by Bacon-Rajan:

- leaf `String` values, because they still have no outgoing refcounted edges

Trigger model:

- the VM buffers possible cycle roots when a graph node's strong count drops but does not hit zero
- collection runs explicitly via `corvid_vm::collect_cycles()`
- auto-collection uses the roots-buffer threshold from `CORVID_VM_GC_TRIGGER`
- `CORVID_VM_GC_TRIGGER=0` disables the auto trigger

Parity model:

- native and VM heaps are still separate implementations
- parity is asserted by tests, not by sharing allocator/runtime code
- current cycle parity is synthetic heap parity, not source-level parity, because Corvid source still cannot mutate fields to construct a cycle directly

## Memory Foundation Retrospective

This is where Corvid stopped being "a language with some RC plumbing" and became a language with a measurable memory story.

What users get now that they did not have before:

- typed native heap objects with per-type metadata
- native cycle collection for unreachable refcount cycles
- interpreter-tier cycle collection, so REPL and replay do not quietly diverge from native semantics
- weak references with checker-enforced effect invalidation rules
- replay-deterministic GC trigger logging
- runtime ownership verification with blame PCs

### What the measurements currently support

These numbers are the current pre-`.6d-2` baseline from `cargo bench -p corvid-runtime --bench memory_runtime ...`. The final foundation lock reruns the same harness after the unified ownership-pass cleanup lands.

| Claim | Supporting number |
|---|---|
| Native fixed-size allocation is now a real competitive strength | `tight_box_alloc`: about `30.6 ns/alloc` hot, `37.9 ns/alloc` after deterministic cold-cache preload |
| Native cycle collection scales cleanly with heap size | mark-sweep stays around `13–17 ns/node` across the current pooled runtime path |
| Runtime ownership verification is no longer prohibitively expensive | verifier `warn/off`: about `1.22x` on `tight_box_alloc`, `1.26x` on `string_heavy_concat`; list-heavy is noise in the current run |
| Future ownership optimizations still have a real baseline to beat | isolated RC ops: about `4.85–5.30 ns` for retain/release and `3.82 ns` for a retain-release pair in the current harness |

### Why this matters for Corvid's positioning

Corvid's moat is not "faster than Rust at everything." The stronger claim is narrower and better:

- replay-deterministic execution
- low audit cost
- ownership verification tied to the runtime's real heap graph

This is the first point where that claim has a measured baseline instead of architecture prose.

### What still depends on the optimization wave

The optimization wave should move the numbers from "credible" to "competitive":

- the unified ownership pass
- pair elimination
- drop specialization
- effect-row-directed RC
- latency-aware RC across tool / LLM boundaries

Those slices matter because they attack the measured overhead directly, not abstractly.

### Pair elimination, first cut

Slice `17b-1c` adds the first explicit retain/release pair-elimination pass to native codegen.

What it does today:

- looks only at same-block `Dup` / `Drop` pairs inserted by the ownership pass
- removes the pair when one safe internal use sits between them and nothing else touches the local
- refuses to pair across branches, loops, agent/tool/prompt/unknown calls, or weak-reference creation

Why the scope is narrow on purpose:

- it is sound without needing to reopen the active CFG/dataflow files
- it documents the safepoint argument explicitly, so GC behavior stays auditable
- it gives Corvid a real ARC-style optimization stage without pretending the broad SSA version already exists

Important measurement note:

- the current `baseline_rc_counts` workloads do not yet expose a same-block removable pair
- so this slice lands with a benchmark-shaped proof fixture and a no-op result on the current published baselines
- the honest next step is to rerun after `.6d-2b` and extend the RC-count suite with a workload that actually exercises pair pressure

### Effect-typed scope reduction

Slice `17e` adds the first effect-aware ownership optimization pass in native codegen.

What it does today:

- looks at `Drop` placement after the unified ownership pass and pair elimination
- classifies statements as either effect-free or effect barriers using a codegen-local sidecar keyed by `IrPath`
- moves `Drop` earlier only when the path between the defining `Let` and the existing `Drop` stays within one straight-line block and crosses no effect barrier

What counts as effect-free in the current slice:

- literal-producing expressions
- local reads
- unary arithmetic
- binary arithmetic

What counts as a barrier:

- all calls
- `approve`
- `if` / `for`
- `return`
- `break` / `continue`
- `Dup` / `Drop`

Why the slice matters:

- it is the first ownership optimization that uses Corvid's effect-awareness rather than only plain liveness
- it shrinks RC-alive windows without pretending the full interprocedural effect-row story already exists

Important honesty note:

- the first post-17e benchmark rerun showed a full-sheet slowdown, including primitive-only paths
- that is treated as environment noise until proven otherwise and is not folded into the published foundation numbers
- 17e ships on correctness first; measurement delta is held until a clean rerun

### Latency-aware RC at prompt boundaries

Slice `17b-7` narrowed a broad intuition into a precise optimization target.

The original hypothesis was "AI boundaries are expensive; optimize tool/LLM boundaries." The implementation work showed that this was too coarse:

- borrowed-local tool args were already close to flat after the unified ownership pass became default-on
- the remaining boundary RC traffic lives in prompt / LLM interpolation, specifically when a borrowed local `String` is threaded through prompt rendering

The shipped pass therefore does one thing on purpose:

- pin borrowed local `String` bindings across prompt lowering so the concat path does not mistakenly release the binding's structural `+1`

What it does **not** do:

- no runtime deferred-RC ledger
- no verifier bookkeeping change
- no attempt to optimize prompt-internal owned temps
- no claim that tool-only workflows materially move from this slice

That is a useful lesson from the memory-foundation work: the moat claim has to follow the measured hotspot, not the broader story we hoped would be true. For Corvid, the differentiated boundary optimization is prompt / LLM lowering, not generic tool dispatch.

### Comparative benchmark runners

The memory-foundation close does not stop at internal microbenchmarks.

Corvid now has a shared AI-workflow fixture set plus three benchmark runner surfaces:

- native Corvid under `benches/corvid/`
- stdlib Python under `benches/python/`
- Node/TypeScript under `benches/typescript/`

The key rule is fixed across all three:

- orchestration overhead equals measured wall time minus fixture-declared external wait

That matters because it keeps the claim honest. Corvid is trying to beat orchestration stacks assembled from libraries, not the network. The benchmark suite therefore measures what the runtimes contribute around prompt, tool, approval, retry, and trace boundaries rather than celebrating whichever runner happened to sleep less.

### Clean-run gate discipline

The first close-out rerun for `memory_runtime` was intentionally archived instead of published.

Why:

- the machine produced runs that disagreed across the full sheet, including one run that passed the primitive-control sentinel while still diverging materially elsewhere
- that is exactly the kind of result that looks tempting in a slide deck and is poisonous in a reproducible benchmark story

So the rule for the close-out is now explicit:

- preserve noisy runs as artifacts
- document the rejection reason
- do not promote them into the published results table until the session clears the quiet-host gate

### Same-session ratio publication

The published close-out numbers use a stricter rule than the earlier absolute microbenchmarks:

- run Corvid, Python, and TypeScript back-to-back in one interleaved session
- subtract fixture-declared external wait from wall time on every trial
- publish ratios and confidence intervals, not absolute milliseconds

That choice matters because this host was not quiet enough to support honest absolute timing claims. The published archive under `benches/results/2026-04-16-ratio-session/` therefore says one precise thing:

- Corvid is slower than both Python and TypeScript on the current comparative runners, and every reported confidence interval stays above `1.0`

That is not flattering, but it is the right close-out claim. The value of the slice is methodological:

- the cross-language benchmark surface is now real
- the subtraction rule is fixed
- the ratio archive is reproducible
- future optimization work has a defensible comparative baseline instead of an aspirational claim

### Internal timing and benchmark-path reductions

The first honest comparative sessions still left one asymmetry in place:

- Python and TypeScript reported in-process trial elapsed time
- Corvid was still paying parent-runner stdin/stdout transport around each
  measured trial

That mismatch is now removed.

The current runner discipline for Corvid native is:

- keep the persistent native process
- measure `wall_ms` inside the launched native benchmark process from trial
  start to trial completion
- subtract actual measured external wait

The measured Corvid path also now avoids benchmark-only overhead that was not
part of the workload itself:

- disabled tracing skips event construction entirely
- trace writes are buffered instead of flushed on every event
- fixture tools use direct typed wrappers
- mock prompt calls avoid decoding and concatenating strings that the mock path
  never consumes

Current same-session result:

- Corvid is faster than the current Python and TypeScript benchmark runners on
  the four shipped workflow fixtures

That statement is intentionally narrow:

- it is a ratio-only result on a noisy host
- it applies to the shipped fixture workloads, not every possible orchestration
  workload
- it is the correct developer and marketing claim only because the earlier
  harness artifacts are now explicitly archived and superseded, not erased

### Compile-time constant prompt rendering

Native prompt lowering now folds a prompt call down to one immortal string
literal when every interpolated argument is a compile-time string / int / bool
literal.

What that means in practice:

- no runtime stringify for those arguments
- no runtime concat chain for the rendered prompt
- the native binary calls the prompt bridge with one pre-rendered literal
  instead of rebuilding the same text every trial

Why it matters:

- several shipped benchmark workflows contain constant prompt calls
- after the internal-timing alignment, those prompt rebuilds were one of the
  clearest remaining avoidable costs
- the new same-session session improves again on both Python and TypeScript,
  especially on the more prompt-heavy workflows

### Professional naming in source

Source code now uses behavioral names rather than roadmap numbering.

What changed:

- benchmark targets use names like `memory_runtime` and `native_foundation_benchmarks`
- inline comments and public API docs describe the behavior directly
- roadmap / slice terminology stays in planning and retrospective documents, not in compiler or runtime source

Why it matters:

- code should read on its own merits
- public API docs should describe the feature, not the project-management history behind it
- roadmap identifiers still exist where they are useful: `ROADMAP.md`, `dev-log.md`, `learnings.md`, and the close-out / deferral docs

### Residual native hot-path profiling

Once startup cost, wait-accounting bias, and benchmark-path overhead were out
of the way, the remaining native orchestration cost turned out to be much
smaller than the earlier investigation suggested.

What the residual profiling slice found:

- the remaining benchmark-path orchestration bucket is already sub-millisecond
  on all four shipped workflows
- bridge / string-conversion work is the largest named remaining component
- prompt rendering, mock dispatch, and release-path time are all small in
  absolute terms
- the unattributed remainder is still a large share of the now-tiny total, but
  only a few hundredths of a millisecond in absolute terms

Why it matters:

- this is the point where micro-optimization stops being the obvious next move
- if we chase another benchmark-only win, the bridge path is the only sensible
  near-term target
- otherwise the correct engineering decision is to move on, because the
  residual cost is no longer large enough to dominate the shipped workflow
  fixtures

### Scalar prompt bridge fast path

The residual profile correctly identified the bridge / string-conversion path
as the last named benchmark bucket worth attacking on the shipped fixtures.

What changed:

- scalar prompt returns under the shipped env-mock path now bypass the generic
  prompt bridge and parse directly from a borrowed queued reply
- profiling-off runs cache the profiling guard state instead of checking the
  environment on every hot-path call

Why it matters:

- this is a real measured-path reduction, not another harness rewrite
- it improves all four shipped workflow scenarios again after the
  constant-prompt pass
- once the residual bucket is already tiny, the winning optimization is often
  removing a whole layer of generic machinery rather than shaving a few
  instructions inside it

### Immortal fixture-string reuse

Once the scalar prompt bridge was out of the way, the remaining fixture-path
overhead lived in ownership churn on canned prompt and tool replies.

What changed:

- repeated env-mock prompt replies are now interned to immortal
  `CorvidString` values
- benchmark tool replies use the same immortal-string path
- the shipped workflow fixtures therefore stop paying per-use release/free work
  on repeated canned replies

Why it matters:

- this is the kind of micro-optimization that is only worth doing once the
  benchmark path is already very small
- it confirms that the remaining hot-path work was in bridge ownership, not in
  prompt rendering itself
- it strengthens the fixture-scoped benchmark claim again without changing the
  measurement methodology

### RC/GC tuning assessment

Once the benchmark-path orchestration cost became small, the remaining question
was whether refcounting or the native cycle collector would become the next
obvious bottleneck under heavier allocation pressure. The answer is "not yet."
The stress matrix stays linear through `100000` releases per trial, the
ownership pass still suppresses retains to `0`, the default GC cadence remains
reasonable on the immediate-release shape, and the native cycle collector
handles `10000` mutual-reference pairs without a pathological spike. That
means RC/GC tuning is not the next roadmap lever; the evidence says to move on
to codegen quality / hot-loop analysis instead of spending another slice on
collector micro-tuning.

### Codegen quality / hot-loop assessment

The right time to do machine-code investigation is when the workload actually
contains a hot loop. The shipped benchmark fixtures do not: they are short
prompt/tool orchestration sequences. The native build is already using
optimized settings (`opt_level = "speed"`, release `opt-level = 3`, thin LTO),
and representative disassembly of the shipped binaries shows dense bridge/helper
call sequences rather than a compute-heavy loop body. That means codegen
quality is not the next benchmark lever for the current workflow sheet.
Machine-code tuning should be revisited when Corvid adds compute-heavy
benchmarks, not treated as the default next step just because other obvious
bottlenecks have already been removed.

### Native nullable `Option<T>` subset

The first honest native step for the Result/Option/retry family was not the
whole feature set. It was the subset the backend already represented cleanly:
nullable-pointer `Option<T>` where `T` is already a refcounted native payload.

What changed:

- the driver's native-ability scan now accepts `Option<String>` / similar
  nullable-pointer payloads
- wide tagged-union shapes like `Option<Int>` still reject cleanly
- parity coverage now proves helper agents can return `Option<String>` and
  wrapper agents can compare the result against `None`

Why it matters:

- this moves real native capability forward without pretending `Result`, `?`,
  or retry are already done
- it confirms the right strategy for the broader feature wave: land the
  genuinely supported subset first, then widen from proven machinery
- the slice also flushed out a real runtime-link contract bug
  (`corvid_bench_tool_wait_ns` missing from the FFI bridge), which is exactly
  why capability work needs end-to-end parity coverage and not just scan tests

### Native nullable `Option<T>` `?` propagation

Once native nullable `Option<T>` existed as `pointer-or-null`, the next sound
step was not more constructors. It was control flow: make postfix `?` work on
that exact representation and no more.

What changed:

- native codegen now treats `Option<T>?` as a null check plus early return when
  the enclosing function also returns a native nullable `Option<_>`
- the early-return path uses the same live-local cleanup walk as explicit
  `return`, so the new control-flow form stays ownership-correct
- native-ability accepts that subset and parity tests prove both `Some` and
  `None` propagation through helper agents

Why it matters:

- it turns native nullable `Option<T>` into a real internal control-flow type
  instead of a value-only curiosity
- it proves the broader feature wave should keep following the same pattern:
  widen from an already-proven runtime representation rather than trying to
  land `Result`, `?`, and retry as one opaque monolith
- it preserves the no-shortcuts rule: the slice still refuses `Result<T, E>`
  and retry until their layouts and control flow exist for real

### Native one-word `Result<T, E>` subset

The honest way to add native `Result<T, E>` was not "declare the whole feature
done." It was to pick a representation the backend can actually own today and
land that end to end. Corvid now lowers one-word `Result<T, E>` shapes as
typed heap wrappers with a fixed `[tag | payload-slot]` layout, plus emitted
destructor/trace/typeinfo metadata so RC and cycle collection see them as real
heap objects rather than a codegen special case. The first test pass exposed
the load-bearing integration point: the unified ownership analysis still
classified `Result<T, E>` as non-refcounted, so result locals leaked even
though the wrapper codegen was otherwise correct. Fixing the analysis, not
papering over the leak in codegen, was the right move. The resulting native
subset is credible: construction works, same-shape `?` propagation works, and
the feature participates in the existing ownership/runtime model instead of
bypassing it.

### Native `Result<A, E>?` to `Result<B, E>`

The next real step after same-shape `Result<T, E>?` was not a bigger wrapper
layout. It was the standard propagation rule users actually expect:
`Result<A, E>?` inside a function returning `Result<B, E>`. Corvid now does
that by rebuilding only the `Err` wrapper on the early-return path. That is
the important design point: the payload representation was already good enough;
the missing piece was a principled ownership-preserving conversion path between
two concrete result wrappers with the same error type. The widening slice
confirmed the same lesson again: once the representation is sound, the hard
part is preserving ownership and cleanup invariants during control flow, not
inventing more layout machinery.

### Native `try ... retry` for `Result<T, E>`

The honest first native retry slice was not "retry anything that can fail."
Compiled Corvid cannot catch process-level traps the way the interpreter can
catch `InterpError`, so the sound AOT subset is retry over the recoverable
native `Result<T, E>` path. Corvid now lowers that subset as explicit native
control flow: evaluate the body, branch on the result tag, release failed
wrappers before the next attempt, compute a deterministic linear/exponential
delay from the source backoff policy, sleep, and re-enter the body. That is
the right shape for future widening because it keeps retry as compiled control
flow rather than hiding it in one opaque runtime helper. The slice also
reconfirmed the testing rule: compile acceptance was not enough. The feature
needed queued mock replies to prove the native tier actually performed multiple
attempts and returned the final `Err` value without silently propagating or
leaking between attempts.

### Native wide scalar `Option<T>`

The honest next widening step after native nullable `Option<String>` was not
to pretend every `Option<T>` is already cheap and native. Corvid now supports
wide scalar `Option<Int>`, `Option<Bool>`, and `Option<Float>` by giving
`Some(...)` a tiny typed heap wrapper while keeping `None` as the zero pointer.
That matters because it preserves the same ownership and collector story as the
rest of the native runtime: the value is a real heap object with typeinfo when
it needs storage, not a codegen-only special case. The slice also exposed a
real generic bug that had been latent before: non-string binary ops were not
releasing refcounted operands after comparison/arithmetic. Wide `Option<T>`
surfaced that immediately through `value != None`, and fixing the generic
lowering path was the right move. The lesson is the same as the earlier native
`Result` work: widening support safely depends less on inventing clever
representations and more on making every new representation participate in the
existing ownership model without exceptions.

### Compositional native tagged unions

The next honest question after landing native `Result<T, E>`, wide scalar
`Option<T>`, and native retry was whether those pieces actually compose or only
work as isolated leaf features. Corvid now has explicit coverage proving that
`Result<Option<Int>, String>` works natively through construction, postfix `?`,
and deterministic retry without any new runtime machinery. That is an important
signal: the current one-word tagged-union representation is not just "barely
enough for the demo cases." It is compositional inside the subset it claims to
support. The lesson is strategic as much as technical: once representation,
typeinfo, ownership, and cleanup invariants are sound, widening support should
first look for shapes that naturally compose out of those primitives before
adding new special-case encodings.

### Wider native `Option<T>?` propagation

The next useful native widening was not a new runtime object shape at all. It
was removing an artificial restriction in `Option<T>?` propagation. Corvid now
lets `Option<T>?` early-return into any native `Option<U>` envelope, because
the `None` path does not care what the eventual `Some(...)` payload type is.
That matters because it keeps the widening semantic rather than representational:
the runtime already knew how to represent these options, and the control-flow
rule was simply narrower than the underlying model required. The same slice also
confirmed that retry composes one step further than the earlier minimal proof:
retrying a native `Result<A, E>` and then using `?` into `Result<B, E>` works
without new runtime machinery. That is the pattern to keep following. Widen the
native subset first where the representation already composes cleanly, then add
new encodings only when a real semantic need remains.

### Native option envelopes and retry composition

The next confirmation after widening `Option<T>?` was whether that broader rule
still held up when mixed with native retry and widened `Result` propagation. It
does. A retried native `Result<String, String>` can now flow through `?` into
`Result<Bool, String>` without new runtime machinery, and `Option<T>?` can
early-return `None` into any native `Option<U>` envelope because the control
flow only needs the envelope's `None` representation. That is the deeper rule:
when a branch of the semantics is payload-agnostic, Corvid should not keep a
same-shape restriction just because it was easier to implement first. The right
pattern is to prove the broader semantic rule once the representation and
ownership model are already strong enough to carry it.

### Structured native `Result` payloads can already ride the current subset

The next honest widening question was whether native `Result<T, E>` really only
handled leaf payloads, or whether the current subset already carried structured
payloads and simply lacked proof. The answer is the latter. Corvid now has
explicit native coverage for `Result<Boxed, String>` and
`Result<List<Int>, String>`, including postfix `?`, without any new runtime or
codegen machinery. The only thing blocking the list case was a frontend bug:
`List` had dropped out of the resolver's built-in generic heads, so
`Result<List<Int>, String>` died before native lowering ever ran. Fixing that
was the right move because it exposed the real property of the system: the
existing one-word native `Result<T, E>` subset already composes with structured
heap-backed payloads that participate in the ownership model. The lesson is to
prefer semantic proof over premature representation work. Before inventing a
new layout, first ask whether the current representation already supports the
broader case and simply lacks tests or a clean frontend path.

### Nested native `Result` payloads should be proven before new layouts are invented

The next meaningful widening after `Result<Struct, String>` and
`Result<List<Int>, String>` was not another leaf payload. It was whether native
`Result<T, E>` still behaved coherently when one side was itself another native
`Result`. Corvid now has explicit proof for nested ok-payloads
(`Result<Result<Int, String>, String>`) and nested error payloads
(`Result<Int, Result<String, Bool>>`), including widened postfix `?` where the
enclosing function changes the ok type but preserves the nested error shape. No
runtime change was needed. That matters because it says the current wrapper,
typeinfo, and ownership model are not just good enough for isolated examples;
they compose one level deeper without new machinery. The lesson is strategic:
before inventing a broader native tagged-union layout, first prove how far the
existing one already goes under realistic composition.

### Build unblocks should complete an unfinished front-end path, not paper over it

This same slice also hit a front-end problem unrelated to native lowering: the
lexer and AST already knew about `effect` declarations, `uses` clauses,
`@constraint(...)`, and cost literals, but the parser still had missing and
duplicate method paths for them. The right fix was to finish that parser path
coherently, not to stub around it just enough for one test to compile. Once the
parser, keyword tests, and declaration recovery all agreed on the same syntax,
the native work could continue without dragging a half-wired front-end branch
forward. The lesson is simple: when an unblock reveals a subsystem that is only
partially switched over, complete that subsystem to one internally consistent
state instead of layering more local exceptions on top.

### Retry should follow Corvid's actual failure carriers, not a narrower implementation subset

Once native `Result<T, E>`, native `Option<T>`, and postfix `?` were all
shipped, retry remaining `Result`-only was too narrow. In the language model
Corvid already exposed, both `Err(...)` and `None` are first-class "did not
produce a usable value" branches. Corvid now makes that explicit: the
typechecker accepts `try ... on error retry ...` only on `Result<T, E>` and
`Option<T>`, the interpreter retries on `Err(...)` and `None`, and native AOT
does the same for the shipped `Option<T>` subset. The lesson is that widening
Phase 18 correctly is often about aligning semantics across the language,
interpreter, and native tier, not just adding one more native representation
case. If a construct is already a language-level failure carrier, retry policy
should treat it coherently across both tiers.

### Prod traces are a regression suite the moment the harness can dispatch them

Corvid now treats every recorded `.jsonl` trace under a directory as a
regression test: `corvid test --from-traces <dir> --from-traces-source <file>`
loads + schema-validates each trace, applies the coverage filters
(`--only-dangerous`, `--only-prompt`, `--only-tool`, `--since`, `--replay-model`,
`--flake-detect`), and dispatches each surviving trace through the regression
harness. Exit code 0 means every trace still behaves the way production
behaved; exit code 1 flags drift. `--promote` now closes the loop: on a TTY it
prompts per divergence and atomically rewrites the golden trace when accepted;
in non-interactive pipelines it fails closed with a one-time warning. The
lesson is that a CLI that only previews the plan is half a feature. Phase 21's
invention is that *production traffic is the test suite*, and that only becomes
real when the CLI actually runs the traces against the current binary, prints a
per-trace verdict, and — when behavior genuinely changed for the better — lets
the operator promote the current run to the new golden instead of having them
re-record by hand.

### The flagship PR-review tool is itself a Corvid program

Corvid now ships `corvid trace-diff <base-sha> <head-sha> <path>`, a
git-integrated behavior-diff tool whose reviewer agent — the piece that
walks two ABI descriptors and emits a markdown PR behavior receipt — is
written in Corvid, not Rust. The `.cor` source lives at
`crates/corvid-cli/src/trace_diff/reviewer.cor`, is embedded into the
CLI binary via `include_str!`, and compiles + runs through the
interpreter on every invocation. The reviewer is `@deterministic`
(byte-identical receipts across reruns), declares its own
`AgentSummary` and `Descriptor` types that the Rust CLI coerces into
via `json_to_value`, and owns the diff logic itself — Rust is
plumbing (git, compile, descriptor extraction) but the "what changed,
and how do we render it" logic is the Corvid agent. That matters
because Corvid's thesis is that AI-native governance is a first-class
programming domain with compile-time guarantees; shipping the flagship
governance tool in the host language would have softened the thesis
the same way Python shipping its linter in bash would. Writing it in
Corvid forced one honest early finding about language scope: there's
no `Float→String` primitive today, which is why slice 1 omits cost
deltas from the receipt (the reviewer will surface cost changes once
the language grows one). Dogfooding surfaces language gaps by
construction, which is exactly the v1.0 polish loop we want.

### ABI-descriptor-as-behavior-surface gives PR review a principled scope

The receipt compares exactly the `pub extern "c"` exported surface and
its transitive closure — the same scope 22-B's `emit_abi` uses for the
ABI descriptor. That is not an arbitrary cut. The exported surface is
what a host actually consumes; it is also what users audit in a PR
because it is the contract that leaves the compilation unit. Private
helpers change often and don't change the host's view; comparing them
would produce noisy receipts that cry wolf. Keeping the scope aligned
with the ABI descriptor means a Corvid program's reviewable surface is
exactly the surface a host relies on, which is the principled answer
to "what should PR-level behavior-diff show." The lesson: when
inventing a review tool, anchor its scope to a pre-existing, defensible
boundary the rest of the system already respects — not a new boundary
invented for the tool.

### Jest-snapshot promotion needs a fresh-run driver helper, not a replay-with-different-flags

A `--promote` implementation that only adjusted replay's substitution knobs
would have been a shortcut. Promotion is semantically the opposite of replay:
replay *substitutes* recorded responses to verify the current code still
produces them; promote *ignores* recorded responses so a fresh run under the
current code + real adapters can overwrite the golden. The right shape is a
sibling driver helper (`run_fresh_from_source_async`) that extracts the trace's
agent + args, compiles the current source, builds a runtime with
`.trace_to(emit_dir)` and *no* replay configuration, runs the agent, and
returns the emitted trace path. The harness then atomically swaps the old
golden for the new one when the operator accepts the divergence. The lesson
generalises: when two behaviours look like knobs on the same pipeline but have
inverse semantics, shipping them as sibling helpers beats shipping them as
modes of one helper. The codepaths read cleanly, the tests cover each property
in isolation, and a future reader doesn't hunt through a single helper to
work out which branch handles which story.

### A sync CLI wrapping an async runner needs two driver helpers, not a nested block_on

The regression harness raises async runner requests (one per trace, each async
because replay dispatches through mock + real LLM adapters). The CLI is
fundamentally sync — `anyhow::Result<u8>` — so it wants to call
`tokio::Runtime::block_on` once at the top. But the runner closure inside the
harness is itself async and dispatches into the replay orchestrator, which was
originally a sync function that did its own `block_on` internally. Nesting a
`block_on` inside another `block_on` panics. The fix that stays honest is to
split the driver helper into a sync wrapper and an async variant
(`run_replay_from_source_with_builder` + `_async`), push all runtime
construction up to the CLI boundary, and let every level below the CLI stay
async. The lesson is that "just call `block_on` again" is a shortcut with a
runtime-panic price tag; if a crate offers a sync helper that other callers
rely on, the answer is to expose the async variant alongside it, not to thread
runtimes through function bodies.

### Visibility-before-imports: ship the rule first, then the mechanism

`lang-pub-toplevel` extended `public` / `public(package)` to
top-level `type` / `tool` / `prompt` / `agent` declarations —
private-by-default, backward-compatible with every existing
single-file program. The rule shipped **before** the mechanism
that makes it load-bearing (`lang-cor-imports-basic`).

Why that order: when imports land, every existing `.cor` file
needs to decide which of its declarations are importable. If we
shipped imports first, the entire ecosystem would be implicitly
public until each file was migrated — exactly the default-public
regret Python has lived with for 30 years. Shipping the rule
first means every file defaults to the right answer (private);
library authors opt in to `public` intentionally when they want
something importable.

The lesson generalises: when adding a language feature whose
semantics depend on a classifier (public/private, safe/unsafe,
pure/effectful), ship the classifier first with the conservative
default, then ship the mechanism that makes the classifier
load-bearing. Users migrate into intentional choices instead of
migrating out of an accidental anti-pattern.

### Honest names are load-bearing before algebraic composition

`21-inv-H-5-schema-fix` was a five-line pre-slice to `-stacked`:
rename `agent.approval.tier_weakened:` to `...tier_changed:` (same
for `reversibility_weakened:` → `...reversibility_changed:`),
bump `RECEIPT_SCHEMA_VERSION` 1 → 2. The delta emitter had always
fired on *any* transition — strengthenings were shipping under a
key called "weakened." The policy layer parsed the `from->to`
suffix so the gate behaved correctly, but the key name lied about
what it represented.

That lie was fine in isolation — no consumer of a single-PR
receipt was harmed by a misleading key. It becomes load-bearing
the moment you try to *compose* receipts. A stack receipt reasons
about the net algebra of N per-commit deltas; the first question
the composer asks is "does `tier_weakened:A→B` cancel with
`tier_weakened:B→A`?" The honest answer is yes (they're a
round-trip). But with the old names, the composer would be saying
"two weakenings cancel," which reads as nonsense and bakes the
naming bug permanently into the algebra's explanation of itself.

Fix the names *before* piling composition semantics on top. The
hardest rule — no shortcuts — includes naming shortcuts taken in
earlier slices, even when the shipped behavior is correct. The
trigger to look was the design question "do strengthenings emit a
delta?" asked in the middle of pre-phase chat for `-stacked`.
Without that audit, `-stacked` would have inherited the lie.

Schema bumps are load-bearing signal, not ceremony. v2 tells bots
pattern-matching on `agent.approval.tier_weakened:` prefixes:
*audit your matchers.* Without the bump, consumers would silently
stop matching anything after the rename. With it, they get a
clear "schema changed" signal they can pin on.

### CI-specific formats are renderers over the canonical Receipt

`21-inv-H-5-gitlab` added `--format=gitlab` in a single commit —
~165 lines of renderer + 6 integration tests — because the
canonical `Receipt` struct H-5 chose was already the source of
truth. Every CI-specific format is "translate deltas + the
verdict into the shape this CI expects"; no pipeline rewiring.
GitLab's MR widget pulls findings from CodeClimate-compatible
JSON (`artifacts.reports.codequality`), so the renderer emits
exactly that shape and disappears into GitLab's native surface.

Three small choices carry the weight:

- **Severity maps one-to-one from the default policy's verdict.**
  Regressions → `major`, everything else → `info`. We picked
  `major` (not `blocker`) so the MR widget surfaces findings
  without overriding the MR's own merge gate — the policy's
  non-zero exit code is what actually blocks merges; severity is
  for signal, not enforcement. The five-level CodeClimate scale
  leaves room to split regressions further in a future slice.
- **Fingerprint = hex-SHA256 of the canonical delta key.** This
  is the GitLab-specific answer to the same "byte-stable across
  runs" principle H-5-json chose with deterministic ordering.
  GitLab dedupes MR-widget issues by fingerprint; if fingerprints
  drift between re-runs, reviewers see phantom "new" findings on
  every push. The fingerprint_stays_stable_across_runs test is
  the regression guard that would catch a drift at commit time.
- **`GITLAB_CI=true` auto-selects the renderer.** `--format=auto`
  now has two CI-platform branches (GitHub Actions →
  `github-check`, GitLab CI → `gitlab`) before falling through to
  the pipe/tty default. Users drop `corvid trace-diff ...` into
  a GitLab job without touching `--format`; the CLI reads
  environment and does the right thing.

The same pattern extends trivially to any future CI surface
(CircleCI, Buildkite annotations, Azure Pipelines): add a
renderer, add an env-var branch in `detect_from_environment`,
done. The canonical Receipt absorbs the new integration as
another translation target rather than a new pipeline.

### in-toto integration: the attestation ecosystem is a free composition

`21-inv-H-5-in-toto` shipped in one commit because H-5-signed
already chose DSSE. Wrapping the Receipt in an in-toto Statement
v1 and swapping the DSSE envelope's `payloadType` to
`application/vnd.in-toto+json` is all it took — cosign,
slsa-verifier, and the rest of the in-toto ecosystem now consume
Corvid receipts natively with zero adapter code.

The lesson is the same one H-5-signed named in reverse:
*choosing the ecosystem-standard format at the base layer pays
compounding interest at every higher layer.* We didn't have to
invent in-toto compatibility — we declared the subject, the
predicateType, and the Statement wrapper, and the format
committee's prior work did the rest.

Design choices worth naming:

- **Subject = reviewed artifact, not the receipt itself.** The
  attestation is *about* the head source file (`sha256` in the
  subject's digest field). Self-attesting would have been
  redundant with H-5-signed's content-hash addressing, and
  would have confused consumers expecting the subject to point
  at the reviewed-thing.
- **PredicateType is Corvid-specific, not SLSA Provenance.**
  URI `https://corvid-lang.org/attestation/receipt/v1`. SLSA
  Provenance describes build inputs/outputs; Corvid receipts
  describe algebraic-effect deltas. Borrowing the wrong
  predicate type would have confused SLSA consumers.
- **Unsigned in-toto output is allowed.** Forcing `--sign` would
  have excluded pipelines that sign externally with cosign's
  KMS-backed signers or keyless OIDC. The unsigned-then-sign
  flow is a legitimate use case we support by not getting in
  the way.
- **`receipt verify` accepts both payloadTypes transparently.**
  The allow-list grew from one entry to two. Callers don't
  branch on payloadType; they just verify and get the bytes.
  Interpretation (Corvid receipt vs in-toto Statement) happens
  at the consumer layer where it belongs.

### Signed receipts move governance from informational to defensible

`21-inv-H-5-signed` turned the trace-diff receipt from "a text
output the CLI prints" into a signed DSSE envelope that external
tools can cryptographically verify. That's the step that moves
Corvid receipts from "a nice audit log" to "a defensible
artifact" — the kind of thing a regulator or security auditor
can check on their own machine without trusting the developer
who produced it.

Design choices worth naming:

- **DSSE over a hand-rolled format.** The Dead Simple Signing
  Envelope is the format used by Sigstore, in-toto, cosign, and
  every modern supply-chain tool. Adopting it means Corvid
  receipts plug into those ecosystems for free — and the
  follow-up `21-inv-H-5-in-toto` slice becomes just "wrap the
  DSSE envelope in an in-toto Statement." Building our own
  envelope would have been cheaper today and more expensive
  forever.
- **PAE over raw payload.** DSSE's Pre-Authentication Encoding
  adds explicit length prefixes so the signature binds both the
  payload AND its type. Ignoring PAE is a known class of
  signature-transplantation bug where an attacker crafts a
  different envelope that the signature still validates against.
  The DSSE spec exists because the non-PAE version was
  foot-guns; we followed it.
- **ed25519 over RSA / ECDSA.** Smaller keys, smaller signatures,
  deterministic (no per-signature RNG needed), single obvious
  security level. Less to go wrong at every layer from key
  generation to verification. Sigstore's keyless flow also
  defaults to ed25519, so staying there keeps the upgrade path
  cheap.
- **Hash-addressed cache with prefix lookup.** Receipts are
  shell-referenceable by their SHA-256 prefix (min 8 chars),
  the same way Git refers to commits. Operators who stared at
  commit hashes all day don't have to learn a new addressing
  scheme.
- **Key-source precedence: `--sign=<path>` > `CORVID_SIGNING_KEY`
  env var.** Explicit flag wins over implicit env — matches how
  every serious CLI handles auth material. Env var support is
  free and useful for CI; file path is the local-dev ergonomic.

The pattern that generalises: *when a domain has a standard
cryptographic format, adopt it whole rather than reinventing.*
The format committee already considered the attacks; your
"simpler" version will rediscover them the hard way.

### Governance receipts are the audit layer, not just a reporter

`21-inv-H-5` started as "add three output format modes" and was
reframed mid-chat to "the trace-diff receipt becomes the AI-safety
audit artifact of Corvid programs." That reframe changed every
design decision that followed:

- Receipt is a canonical structured object, not a concatenated
  string. Each format (`markdown`, `github-check`, `json`) is a
  view over the same struct. Adding a format is adding a
  renderer, never touching the pipeline.
- JSON output is schema-versioned from day one (`schema_version:
  1`). Bots pin against the version; breaking changes get v2
  while v1 consumers keep working. Schema evolution is a
  first-class commitment.
- Regression policy is its own concern, separable from the
  renderer. Shipping a baked-in conservative default today;
  promotable to a user-replaceable `.cor` program in the
  follow-up slice (`21-inv-H-5-custom-policy`). Governance-as-
  code for the gate itself.
- Exit code is policy output, not a flag. `--gate=on|off` was
  tempting and would have worked; rejected because the gate's
  WHY lives in the policy. Exit 0 on `verdict.ok`, exit 1
  otherwise. Custom policies replace the verdict; they don't
  ask the CLI for permission to fail.

The pattern generalises: whenever a language ships a structured
governance concept (effect algebra, approval contracts,
provenance), the corresponding receipt should be a first-class
audit artifact — structured, versioned, policy-gated, and
eventually signed — not a pretty string. The receipt is how the
compile-time guarantee becomes a durable record the rest of the
world can inspect.

The five follow-up slices filed (`-custom-policy`, `-signed`,
`-in-toto`, `-stacked`, `-watch`, `-gitlab`) each extend this
audit-layer thesis in a different direction. They land
independently because the receipt is structured — no one
follow-up has to know about the others.

### The CTO reframe: scope as leverage, not as a list

When planning `21-inv-H-5` I drafted a conservative chat with
three questions and five implementation decisions. The user asked
me to answer the questions "in the way that makes Corvid powerful
and limitless." That reframe moved H-5 from an incremental
feature to a category-defining one, and it taught an instruction
worth honouring for future planning:

**Default to ambition in design; default to discipline in scope.**
The canonical receipt + policy-as-code is the ambitious design.
The first slice ships the canonical receipt, three renderers,
and a baked-in policy with exit-code gating. Everything else —
`--policy=<path>`, signed receipts, in-toto attestations, stacked
PRs, watch mode, GitLab renderer — is explicitly filed as a
follow-up. The design vision is limitless; the shipping vehicle
is disciplined.

The failure mode to avoid is the opposite: conservative design
("just add format flags") + generous scope ("land all three
modes + signing + in-toto in one slice"). That's the worst of
both — no leverage AND a brittle ship. Ambition in design gives
follow-ups their meaning for free; discipline in scope makes the
current slice shippable.

### Non-deterministic generators, deterministic receipts: the wrapping-layer pattern

`21-inv-H-4` wanted an LLM-generated prose paragraph at the top of
the trace-diff receipt, but the receipt overall still had to be
byte-deterministic where it mattered (CI, `--format=json`). The
conflict is real: prompts produce different strings each run.

The resolution is a wrapping-layer pattern with three tiers:

1. A deterministic orchestrator (`review_pr`) renders the whole
   receipt from structured inputs. It is `@deterministic` —
   identical inputs always produce byte-identical output.
2. A narrow non-deterministic surface (`summarise_diff` prompt)
   produces exactly one piece of the structure: the narrative
   paragraph.
3. A deterministic pre-filter between them (`validate_narrative`)
   enforces strict all-or-nothing rules on the LLM's output; on
   rejection it substitutes a deterministic sentinel.

The critical property: the non-determinism is fenced inside tier 2
and never leaks into tier 1 or tier 3. `review_pr` renders the
narrative OR the boilerplate fallback, and both are deterministic
given their inputs. The caller can *opt into* the non-deterministic
path (`--narrative=on/auto`) or out of it (`--narrative=off`);
opting out makes the whole receipt byte-deterministic again with no
special casing in `review_pr`.

The lesson generalises beyond this slice: any language that wants to
mix LLM surfaces into deterministic artefacts needs a pattern like
this. A pre-phase chat almost settled on `Grounded<ReceiptNarrative>`
for H-4; what actually shipped is ungrounded plus strict
post-validation, because the language couldn't mint a grounded value
from a plain value today. The deferred follow-up is to re-wrap once
22-F lands the provenance-handle path across FFI. The pattern
doesn't change — only the type-level annotation gets sharper.

### Grounding across FFI is a runtime attestation, not an extended type wall

Thinking about how `Grounded<T>` crosses the FFI boundary (for the
post-22-F H-4 follow-up) forced a choice that applies to any
system mixing language-level effect walls with foreign hosts.

Inside Corvid, `Grounded<T>` is an effect wall: you cannot extract
`T` without staying in a grounded context. Across the FFI the host
has `T` the moment it reads the return value. Two options:

1. Grounding is informational at the boundary. Host receives
   `(payload, handle)`; handle queries sources + confidence. The
   Corvid-side guarantee survives as an *inspectable attestation*
   but not as a compile-time wall.
2. Grounding is enforced at the boundary. Host never receives `T`
   directly; only operates on opaque handles through FFI
   primitives.

Option 2 sounds purer but is wrong in practice. It forces hosts to
re-express their entire call graph through grounded-handle
primitives — impractical for real C/Python/Rust hosts — and gives
*false* security because a determined host casts the handle to a
pointer and reads the bytes. Pretending the type wall extends into
foreign code when it can't is worse than admitting it doesn't.

Option 1 is honest: the compile-time wall stays a compile-time
guarantee *inside* Corvid; at the boundary it transforms into a
runtime attestation the host can inspect when it cares. The
attestation surface (source names, confidence, handle lifetime)
lets sophisticated hosts act on the evidence. Hosts that don't care
just use the payload and release the handle.

The deep lesson: when a language-level effect guarantee meets a
foreign runtime, the honest move is to let the guarantee decompose
into runtime evidence rather than try to extend the wall. The same
reasoning will apply to `@dangerous`, `@deterministic`, approval
contracts — each has a compile-time teeth and a runtime receipt, and
each receipt is what crosses the boundary.

What actually shipped in `22-F` follows directly from that choice:

- the host gets `(payload, handle)`, not an opaque grounded-only value
- Level 1 exposes `List<String>` source names plus a confidence query,
  which covers the common host questions without freezing the richer
  internal provenance shape too early
- `0` is the null grounded handle and `release(0)` is a no-op, matching
  normal C conventions
- handle lifetime lives in a slotmap-backed attestation store with a
  generation counter, so stale or double-released handles fail cleanly
  instead of degrading into silent misuse

Just as important was what did *not* ship: host-side grounding minting.
Returning a grounded value from Corvid is **earned grounding** - the
runtime can point at the retrieval/prompt/tool path that produced it.
Letting the host construct a grounded handle is **asserted grounding**,
which needs a separate audit trail (`host_asserted`, provenance
ownership, review semantics). Splitting those into separate slices
keeps the trust model honest instead of blurring two very different
claims behind one ABI.

### Citation validation is what makes grounding meaningful, not decoration

`21-inv-H-4`'s citation rule from the pre-phase chat: all-or-nothing.
Every `delta_key` an LLM cites must be in the allow-list we computed
from the structural diff; a non-empty body with an empty citations
list is rejected; duplicate keys are rejected. Any violation drops
the entire narrative and falls back to boilerplate.

The alternative — partial acceptance — was considered and rejected.
Partial acceptance lets the narrative keep the phrases whose
citations validated and drop the phrases whose citations didn't. But
once the LLM's output has been surgically edited, the sentences that
remain may no longer flow, may reference changes out of order, or
(worst) may preserve a phrase whose cited `delta_key` was valid but
whose *semantic* claim was about a different change entirely. The
citation validates the key; it can't validate the prose. All-or-
nothing keeps the validation's meaning crisp: *either this whole
paragraph is honestly cited, or we don't trust any of it.*

The lesson for any system that mixes LLM text with structured
grounding: the grounding check is only meaningful when it's load-
bearing. A check you only sometimes act on isn't a check, it's
decoration.

### Positional struct constructors, not struct-literal braces

`21-inv-H-3` tried to build a sentinel `ApprovalLabelSummary` inside
the Corvid reviewer with `ApprovalLabelSummary { label: "", ... }` —
which parses as a call followed by a stray `{` and fails with
`unexpected token LBrace`. Corvid user-defined types are constructed
positionally: `ApprovalLabelSummary("", "", "")`. The language didn't
borrow Rust's named-init syntax, and the lesson is worth remembering
in future `.cor` authoring: *struct literal braces are not a Corvid
construct, positional calls are the one shape*. Our existing
`examples/structs.cor` demonstrates this and is the canonical
reference.

### Reachability, not visibility, decides what's in the ABI

`21-inv-H-3`'s integration test needed a non-`pub extern "c"` helper
agent (`explain`) to show a grounded-return transition, because
`Grounded<T>` can't cross the C ABI. The first fixture attempt put
the helper in the source but didn't call it from the exported
`refund_bot` — and the helper silently disappeared from the ABI.
`crates/corvid-abi/src/emit.rs` restricts `abi.agents` to the
*transitive closure of `pub extern "c"` agents* via
`collect_exported_agent_closure`. So for receipt tests (and for the
real-world PR-review workflow), the rule is: if a helper's contract
changes but nothing reachable from an exported agent calls it, the
receipt won't see the change. That is the correct behaviour (dead
code shouldn't pollute the receipt) but worth stating explicitly so
future integration fixtures don't fall into the same trap.

### The Corvid reviewer keeps ownership of structure even when the language lacks Int→String

`21-inv-H-2` wanted to render a "Counterfactual Replay Impact" section
with sentence-shaped summary counts — "Replayed 10 trace(s) against base
and head: 7 passed on both, 2 newly diverged under head, 1 newly passing
(base bug fixes), 0 diverged on both, 0 errored." Corvid doesn't yet have
an `Int→String` primitive. The temptation was to format the whole section
in Rust and pass the fully-rendered block to the reviewer as a `String`,
which would have collapsed the section into a Rust deliverable with the
`.cor` file only responsible for deciding "include or omit." The honest
split keeps the numeric formatting in Rust (where the primitive lives
today) but keeps *structure* ownership in the reviewer: the reviewer
chooses whether the section renders, where it sits in the receipt, what
narrative lines surround the pre-formatted summary, the heading for the
newly-divergent path list, and how the list itself is rendered. The
lesson is durable: when a language gap forces some work out of the
dogfooded layer, push only the narrowest possible piece out and keep
structure ownership where the thesis wants it. A future language slice
that adds `Int.to_string()` will make the reviewer fully self-sufficient
without a receipt layout change.

### Path-list caps in governance receipts protect the reader without losing data

The counterfactual impact report caps the newly-divergent trace path
list at twenty entries, appending an "... (and N more)" row so the
reader always knows the cap fired. The full list will be available in
the 21-inv-H-5 JSON output mode for bots that want it. The lesson
isn't the specific number — it's that governance output intended for
human review needs an explicit cap with a visible truncation marker,
not silent omission. A PR reviewer staring at a list of 600 broken
traces needs a "run the CLI locally for the full list" signal; a list
of 600 names without that signal either drowns the reviewer or gets
scrolled past.

### The spec is a runnable program, not a document

Phase 21's documentation slice follows the pattern already established for
the effect system: every numbered `.md` file in `docs/effects-spec/` is a
program under disguise. Code blocks tagged `# expect: compile` are extracted
by `corvid test spec` and re-compiled against the current toolchain on every
build; a broken example fails CI. Writing [section 14](docs/effects-spec/14-replay.md)
forced an honest audit of which Phase-21 surface is actually demonstrable
*today* (the `replay` language primitive with only the constructs the parser
accepts — no `.is_some()`, no `Int.to_string()`, no list `.push()`) vs. which
parts I was tempted to illustrate with constructs the language doesn't have
yet. Writing the spec as a runnable artefact is also what lets the v1.0
launch demo at `docs/v1.0-demo-script.md` be a script of copy-pasteable
commands rather than a slide deck — every claim resolves to a command whose
output proves it. The lesson is durable across phases: specification work
that ships alongside a "does the compiler still agree with this?" harness
stays honest on its own; specification work that ships as prose drifts away
from the shipped compiler within weeks. For a language whose thesis is
"compile-time guarantees," the spec has to compile.

### Nullable-pointer options are only safe until they stop preserving information

The cheap native encoding for `Option<T>` is a good one when the payload has a
non-null native representation: `Some(payload)` is the payload pointer/value and
`None` is zero. But that encoding is not universally sound. As soon as the
payload is itself an option-shaped value, bare nullability collapses semantics:
outer `None` and `Some(None)` both become zero. Corvid now widens the native
representation at exactly that boundary by allocating a tiny typed wrapper for
nested option payloads while keeping direct nullable-pointer options on the fast
path. The lesson is architectural: representation widening should happen where
the current encoding stops being semantically injective, not just where it is
convenient to add one more case.

### Restricted filter DSLs keep effect queries honest in a way general expression languages do not

`22-D-effect-filter` could have shipped as "evaluate a tiny expression language
over the embedded descriptor" and been superficially more flexible on day one.
That would have been the wrong trade. The host-side question is narrow:
"which capabilities definitely satisfy these effect constraints?" A JSON AST
with only `all`, `any`, `not`, and leaf predicates makes that question explicit
and keeps every failure mode denotationally crisp: unknown dimension is
`UNKNOWN_DIMENSION`, invalid operator for a dimension is `OP_MISMATCH`, malformed
syntax is `BAD_JSON`. A free-form expression language would blur those into a
parser/runtime soup, make host bindings harder to generate, and invite
stringly-typed shortcuts at the FFI boundary. The broader lesson is that when
the domain is a constrained algebra, the honest API is a constrained algebraic
surface too, not a miniature programming language.

### Missing effect fields are a third truth value, not false and not true

The subtle design choice in `22-D-effect-filter` was how to treat agents that do
not declare the field a host queried. Returning true would silently widen
safety-sensitive queries like `trust_tier <= autonomous`; returning false would
make `not { dangerous == true }` look like a sound "definitely safe" query even
though the descriptor never asserted that fact. Corvid's filter now treats
missing fields as a third truth value: the predicate is unevaluable for that
agent, so the agent is omitted from the result set. This keeps narrowing
semantics monotonic and forces hosts that care about the omitted population to
ask a second, explicit question. The durable lesson is that optional metadata in
a safety-facing query surface should usually model "unknown" honestly rather
than collapsing it into either branch of a boolean.

### Effect bounds crossing into runtime should become attestations plus host policy, not runtime-owned walls

`21-inv-H-5`, `22-F`, and `22-G` all hit the same architectural seam from
different directions:

- `H-5` turned the regression gate into a structured receipt plus policy output
  instead of a hard-coded CLI branch.
- `22-F` let `Grounded<T>` cross FFI as `(payload, handle)` so the host can
  inspect provenance evidence without pretending the Corvid type wall survives
  inside foreign code.
- `22-G` applies the same pattern to cost and latency: the runtime records a
  per-call observation handle, exposes realized cost / latency / token counts,
  and reports whether the declared bound was exceeded, but it does **not**
  unilaterally kill the call.

The common lesson is the durable one: when a compile-time effect dimension
meets runtime or FFI, the honest shape is usually **attestation + host policy**.
The language keeps its compile-time guarantee where it is real; the runtime
turns observed reality into structured evidence; the host decides what to do
with that evidence. Trying to extend the compile-time wall wholesale into a
foreign host or a runtime policy engine either centralizes too much policy in
the runtime or offers false comfort when the host can always step around it.

`22-G` makes that concrete:

- one top-level `corvid_call_agent` returns one observation handle
- the handle owns realized `cost_usd`, `latency_ms`, `tokens_in`,
  `tokens_out`, and `exceeded_bound`
- if no cost bound was declared, `exceeded_bound` is simply `false`
- hosts that care about "no bound declared" can already learn that from the
  embedded descriptor and write their own policy on top

That pattern is now the default for future effect-facing Phase 22 / 23 slices:
compile-time algebra inside Corvid, runtime attestations at the boundary,
policy authored by the host unless a later slice makes a very explicit case for
runtime enforcement.

### Computation can be distributed as a replay capsule, not just as code

`22-H-replay-across-ffi` closes the loop on the earlier Phase 21 and 22
guarantees. Corvid already had three strong pieces in isolation:

- a compiled cdylib carrying its full ABI and effect surface
- deterministic replay over recorded traces
- structured receipts and policy outputs about what changed

The important move in `22-H` was to make those one artifact instead of three
related features. A Corvid execution can now be packaged as a **replay
capsule**: library, embedded descriptor, trace, and manifest bound together by
content hashes plus schema/version metadata. That turns an execution into a
portable unit for debugging, audits, regressions, and cross-host reproduction.

Two implementation choices are the durable lessons.

First, host-originated events belong in the same trace stream as runtime
events. `host_event` is not a sidecar file and not a separate schema. The host
submits the event through the ABI, but the cdylib remains the single writer.
That keeps replay, viewers, and tooling working over one timeline instead of
teaching every consumer how to merge multiple partial histories.

Second, determinism at the boundary has to be stated honestly. Corvid now seeds
its own run identity and clock reads from deterministic metadata when replaying
through the FFI, but it does **not** claim control over opaque SDK jitter or
adapter-internal scheduling it does not own. The strong claim is therefore:
seed-deterministic for the Corvid-controlled surface. That is still enough to
make capsules durable and cross-host portable without quietly overstating the
guarantee.

This also extends the same architectural pattern that showed up in `H-5`,
`22-F`, and `22-G`: runtime reality crosses the boundary as structured
evidence, and the host decides what to do with it. `22-H` applies that to whole
executions. The capsule is not just a convenience archive; it is the boundary
artifact hosts can inspect, replay, diff, sign later, and build policy around.

### Corvid does not flatten semantics at the FFI boundary; it projects them into the host as typed constructs

`22-I-host-bindings` could have stopped at a familiar "generate wrappers from a
descriptor" story. That would have produced callable Rust and Python surfaces,
but it would also have thrown away the reason Corvid's ABI descriptor exists in
the first place: the descriptor carries semantic layers, not just transport
types.

The slice becomes interesting only once those layers survive the boundary:

- effect algebra surfaces as host-visible constants and typed catalog queries
- `@dangerous` becomes an `Approver` requirement at the call site instead of a
  runtime convention the host might forget
- `Grounded<T>` becomes a payload wrapper plus provenance access with automatic
  cleanup, not "raw value plus maybe some extra helper functions"
- `Observation` becomes a first-class returned object with RAII/context-managed
  lifetime, so cost and latency evidence are part of the host API rather than a
  leak-prone side channel

The architectural hinge is descriptor-hash drift detection. Generated bindings
embed the descriptor hash they were projected from and compare it against the
loaded cdylib's own `corvid_abi_descriptor_hash` bytes at load time. That keeps
"bindings and library drifted apart" out of the realm of mysterious runtime
misbehaviour and makes it a designed failure mode instead. The host gets a
typed `DescriptorDrift` error before it can make the wrong call against the
wrong binary.

Two broader lessons fall out of that.

First, the source of truth has to stay semantic. The bindings generator reads
the `22-B` descriptor, not the C header. A header can tell you argument and
return shapes; it cannot tell you trust tier, approval contract, replay
metadata, reversibility, grounding, or future effect dimensions without
re-encoding all of that in a second place. Once the descriptor is the only
semantic source of truth, generated bindings stay projections instead of
becoming a second specification.

Second, "idiomatic bindings" is not the same thing as "thin bindings." The thin
approach would have exposed stringly filter JSON, manual handle release, and a
generic load error. The more honest idiomatic approach is slightly thicker: a
typed builder that still lowers to the runtime's JSON DSL, RAII/context-managed
wrappers over existing handle APIs, and a dedicated error variant for drift. In
other words, preserve semantics in the host surface without inventing a second
runtime or a parallel policy engine.

That pattern will matter again for future boundary-facing slices. Once a
language has real semantic layers, the boundary should preserve those layers as
typed host constructs wherever possible, and fall back to structured evidence
when compile-time projection is impossible. Flattening is the easy move. The
interesting move is to keep the language's meaning intact on the far side of
the FFI.

### Corvid makes FFI ownership compile-time-guaranteed instead of host-side convention

`22-J-ownership-check` closes the biggest semantic gap left after `22-I`. The
bindings already gave hosts RAII/context-managed wrappers for grounded values
and observation handles, but the ownership contract behind those wrappers was
still partly implicit: descriptor conventions, destructor naming patterns, and
generator-side knowledge of which handle families needed cleanup.

The important move in `22-J` is not "add ownership annotations." The important
move is to make ownership a structured semantic dimension of the FFI surface.
Extern signatures now carry ownership information through three layers at once:

- the checker infers or validates the contract at compile time and refuses
  ambiguous or unsound extern signatures
- the ABI descriptor carries ownership as typed JSON, including destructor kind
  and symbol when a host must release or drop something
- the Rust and Python generators read that ownership contract directly and emit
  the correct host-side lifetime or cleanup shape from the descriptor instead
  of from naming conventions

That changes the category. Most systems languages stop tracking ownership at
their FFI boundary. Rust's borrow checker is strong inside Rust and then hands
off to `extern "C"` with no semantic guarantee about who frees what on the far
side. Corvid keeps the ownership algebra intact across the boundary: a borrowed
string parameter becomes a borrowed host view, an owned grounded result becomes
a wrapper that knows which release symbol to call, and loosening ownership in a
public extern surface shows up as a receipt delta that policy can flag.

The destructor symbol is the load-bearing detail. Without it, a supposedly
typed ownership system still falls back to convention: guess `corvid_<type>_drop`,
hope the naming pattern survives, and special-case each new handle family in the
generator forever. Once the descriptor carries both the ownership mode and the
actual destructor contract, the generator no longer has to know that grounded
values or observations are special. They become ordinary ownership-projected
descriptor entries.

That is the real lesson from the slice: if a language claims semantic richness
at its FFI boundary, it cannot stop at "safe if documented carefully." The
compiler has to refuse unsound contracts, the descriptor has to encode lifetime
and destruction semantics structurally, and the host bindings have to derive
their cleanup behavior from that structure. Anything weaker is still
convention, just with better comments.

## 22-H-windows-record

Windows caught a real FFI bug that Linux had been letting us get away with.
The generic `corvid_call_agent` path was reusing exported `pub extern "C"`
wrappers as if their ABI were only the user-visible parameters plus the return
value. They were not. Those wrappers also append a hidden observation-handle
out-pointer. Linux happened not to explode when generic dispatch skipped that
pointer; Windows faulted immediately in the direct-observation finish path.

The lesson is narrower and more useful than "Windows is fragile." Shared FFI
dispatch code has to call the real exported ABI, not a simplified mental model
of it. If the wrapper owns observation bookkeeping, every generic call site has
to supply the same hidden out-parameter that a hand-written host would. The
fact that one platform survives undefined behavior is not evidence that the ABI
is sound.

The permanent fix is the guard, not just the patch. This slice added focused
Windows record and replay tests in `corvid-runtime/tests/trace_record.rs` and a
Windows CI job that runs them on every push. That changes the system: the next
time a host-dispatch path drops a hidden ABI parameter or returns a silently
wrong observation handle, the failure shows up in CI instead of months later in
a demo slice.

## 22-K-bundle-public-spec

The bundle story only became launch-grade once the demo stopped being private
scaffolding and became a public spec artifact. A happy-path bundle by itself is
easy to fake. A happy bundle plus typed failing siblings, lineage traversal,
counterfactual query, rebuild verification, and offline audit forces the
implementation to state exactly what it believes a trustworthy Corvid artifact
is.

That changed the quality bar in a useful way. The public surface could not stop
at `verify`; it had to answer six different questions with one coherent model:
is the artifact intact, does it rebuild, what changed from its predecessor,
what approval/provenance semantics are inside, which counterfactual delta
explains the difference, and does the predecessor chain still verify? Once the
examples were committed, any drift between those answers became visible
immediately.

The real lesson is that credibility comes from adversarial siblings, not from
README prose. The neighboring broken bundles matter as much as the happy path,
because they prove the failure boundaries are typed and reproducible: hash
tamper, receipt-signature tamper, rebuild drift, lineage fraud, and unsupported
counterfactual asks all fail for distinct reasons. That is what turns the
bundle format into a one-time public proposal moment rather than a screenshot
of an internal demo.

## 20b-strict-prompt-citations

`cites ctx strictly` is only meaningful if the compiler and runtime agree on
what "ctx" means. The compiler side now rejects strict citation clauses unless
the cited prompt parameter is explicitly `Grounded<T>`, so the annotation cannot
silently attach to an ordinary string with no provenance.

The runtime lesson was sharper: provenance wrappers are not citation text.
Citation verification must inspect the grounded payload, not the JSON envelope
that carries provenance metadata. The same boundary rule applies to tools and
prompts that return `Grounded<T>`: external JSON supplies the inner `T`; Corvid
adds the provenance wrapper after the trusted retrieval or grounded transform
has been established.

Native parity forced the same rule into codegen. `Grounded<T>` is an evidence
type, not a different runtime payload shape on the hot path. Codegen should
lower it as the inner scalar for interpolation, trace payloads, and prompt
bridge calls, then attach or verify provenance at explicit runtime boundaries.
That keeps the native tier behaviorally identical to the interpreter without
inventing a second citation checker in Cranelift.

## 20b-explicit-provenance-discard

Use `.unwrap_discarding_sources()` when a program intentionally drops
`Grounded<T>` evidence and continues with the inner `T`:

```corvid
effect retrieval:
    data: grounded

tool fetch_doc(id: String) -> Grounded<String> uses retrieval

agent export_text(id: String) -> String:
    doc = fetch_doc(id)
    return doc.unwrap_discarding_sources()
```

The method takes no arguments and only exists on `Grounded<T>`. It is not a
runtime conversion in native code; `Grounded<T>` is represented as the inner
payload on the hot path, so the explicit IR node records intent for the
compiler while preserving the efficient ABI shape.

This is a visibility feature as much as a convenience feature. Corvid still
keeps legacy `Grounded<T>`-to-`T` assignability for compatibility today, but
new code should prefer the method so provenance erasure is visible in source,
IR, reviews, and future policy tooling.

## 20d-wrapping-arithmetic

Overflow policy is part of the language contract, not a backend accident.
Default Corvid integer arithmetic should trap because silent wraparound is the
wrong default for safety-oriented agent code. The opt-out is explicit:

```corvid
@wrapping
agent mix(x: Int) -> Int:
    return x * 6364136223846793005 + 1
```

The useful implementation pattern is to preserve intent in IR. Lowering marked
agents into `WrappingBinOp` / `WrappingUnOp` nodes makes interpreter, Python,
native codegen, ABI walkers, and optimization passes handle the policy
deliberately instead of rediscovering it from agent metadata later.

`@wrapping` is deliberately narrow. It applies to integer add/sub/mul and unary
negation; division and modulo by zero still trap. That keeps hash-mixing and
low-level arithmetic possible without turning the annotation into a blanket
"unsafe arithmetic" mode.

## 20e-confidence-gated-trust

`autonomous_if_confident(T)` only matters if it is a runtime boundary, not just
a pretty trust value in the static effect algebra. The checker can treat the
gate's `above` tier as autonomous for compile-time composition, but the
interpreter must still inspect the actual confidence flowing through the call.

The right behavior is conditional approval activation:

```corvid
effect gated_refund:
    trust: autonomous_if_confident(0.90)

tool issue_refund(decision: String) -> Receipt uses gated_refund
```

If the input confidence is `0.95`, the tool runs autonomously. If it is `0.70`,
the interpreter calls the normal approval gate before dispatching the tool.
That preserves the safety model without requiring programmers to write two
manual branches for every uncertainty boundary.

Prompt confidence has to travel with the value. Measuring a prompt result's
confidence but returning a plain `String` loses the exact metadata downstream
confidence gates need. Wrapping low-confidence prompt outputs as `Grounded<T>`
keeps the runtime value carrying its statistical provenance while preserving
ordinary source-level ergonomics.

## 20e-calibrated-prompts

Use `calibrated` inside a prompt declaration when prompt confidence should be
audited against ground truth supplied by an eval runner or adapter:

```corvid
effect confident_model:
    confidence: 0.90

prompt classify(input: String) -> String uses confident_model:
    calibrated
    "Classify {input}."
```

The modifier does not invent correctness labels. It records samples only when
the runtime receives a real correctness observation with the LLM response. That
keeps calibration honest: production calls without labels keep running normally,
while evals and harnesses can accumulate model-level reliability statistics.

The runtime tracks calibration by `(prompt, model)` and reports sample count,
accuracy, mean confidence, confidence/accuracy drift, and a miscalibration flag.
The current flagging rule is intentionally simple: after at least three labeled
samples, drift above `0.25` is considered miscalibrated. Future eval tooling can
render these stats directly instead of treating self-reported confidence as
truth.

## 20e-repl-confidence-step-through

Use `:stepon` or `:stepinto` in the REPL to inspect confidence as the agent
runs. Boundary steps now show input confidence before tool, prompt, and agent
calls, and result confidence after prompt/tool/agent results.

Confidence gates are visible as approval boundaries:

```text
approval required: ConfidenceGate:issue_refund
  confidence gate: actual 0.700 / threshold 0.900 (triggered)
```

That matters because `autonomous_if_confident(T)` is dynamic. A program can be
statically valid and still require human approval on a specific execution if
the values flowing into an irreversible action are less confident than the
threshold. The REPL now shows that threshold comparison directly instead of
hiding it behind a generic approval prompt.

`:trace` includes the same metadata, so the confidence story is visible both
during live step-through and after the run when reviewing the last execution.

## 20f-stream-grounded-provenance

`Stream<Grounded<T>>` preserves provenance per element. Each yielded item is an
ordinary `Grounded<T>` value, so consumers can inspect the source chain of the
specific chunk they received:

```corvid
effect retrieval:
    data: grounded

tool fetch_a() -> Grounded<String> uses retrieval
tool fetch_b() -> Grounded<String> uses retrieval

agent docs() -> Stream<Grounded<String>>:
    yield fetch_a()
    yield fetch_b()
```

The stream itself also exposes an aggregate provenance union. That union grows
as elements are consumed, not when a producer buffers ahead. In REPL
step-through this means a stream local starts with no sources, then shows
`retrieval:fetch_a`, then `retrieval:fetch_a, retrieval:fetch_b` as those
elements are actually delivered.

This keeps streaming provenance honest: the aggregate describes observed
stream content, while each element keeps its precise source chain.

## 20f-mid-stream-model-escalation

Use `with escalate_to <model>` with a streaming prompt confidence floor when a
low-confidence stream should continue on a stronger model instead of failing:

```corvid
model expert:
    capability: expert

prompt draft(ctx: String) -> Stream<String>:
    with min_confidence 0.80
    with escalate_to expert
    "Draft {ctx}"
```

If the initial stream result is below `0.80`, Corvid records a
`StreamUpgrade` trace event and issues a continuation call to `expert` with the
partial output included as context. The stream consumer receives the upgraded
result through the same stream value; the trace shows where the model boundary
changed.

The escalation target is checked like other model-routing features. An
undefined target is a resolver error, and a target that resolves to a non-model
declaration is a type error.

## 20f-progressive-structured-partial-streams

`Stream<Partial<T>>` lets a prompt expose structured output before the full
object is finished:

```corvid
type Plan:
    title: String
    body: String

prompt plan(topic: String) -> Stream<Partial<Plan>>:
    "Plan {topic}"

agent first_title(topic: String) -> Option<String>:
    for snapshot in plan(topic):
        return snapshot.title
    return None
```

For a `Partial<Plan>`, `snapshot.title` has type `Option<String>`. It is `Some`
when the model has completed that field and `None` while the field is still
streaming. The VM schema asks adapters for explicit field states:
`{ "tag": "complete", "value": ... }` or `{ "tag": "streaming" }`.

This is intentionally interpreter-first. Native CL lowering rejects `Partial<T>`
for now, because shipping a real native version needs a dedicated tagged
field-state layout rather than a generic object shortcut.

## 20f-stream-resumption-tokens

`ResumeToken<T>` is the typed checkpoint for interrupted prompt streams:

```corvid
prompt draft(topic: String) -> Stream<String>:
    "Draft {topic}"

agent capture(topic: String) -> ResumeToken<String>:
    stream = draft(topic)
    for chunk in stream:
        break
    return resume_token(stream)

agent continue_it(token: ResumeToken<String>) -> Stream<String>:
    return resume(draft, token)
```

The typechecker requires `resume_token` to receive `Stream<T>` and requires
`resume(prompt, token)` to pair a `ResumeToken<T>` with a prompt returning
`Stream<T>`. The runtime token stores the prompt name, original arguments,
delivered chunks, and an optional provider session handle.

The current implementation is honest about its boundary: provider-native
continuation handles are represented but not fabricated. Until adapters expose
real session state, resume reopens the prompt locally with delivered elements
included as continuation context.

## 20f-stream-fanout-fanin

Corvid stream partitioning is now expressed in the language rather than as
library glue:

```corvid
type Event:
    kind: String
    body: String

agent fanout() -> Stream<Event>:
    groups = source().split_by("kind")
    return merge(groups).ordered_by("fair_round_robin")
```

`split_by` returns `List<Stream<Event>>`, with group order based on the first
time each key appears. `merge(...).ordered_by(...)` supports `fifo`, `sorted`,
and `fair_round_robin`.

The first version deliberately uses string-literal struct fields as key
extractors. That is less general than lambdas, but it is typechecked today and
does not invent an unowned function-value model. Real function extractors belong
with first-class functions.

## 20f-backpressure-propagation

Backpressure is now part of Corvid's language semantics, not just a runtime
queue setting:

```corvid
effect live_feed:
    latency: streaming(backpressure: pulls_from(producer_rate))

prompt watch(topic: String) -> Stream<String> uses live_feed:
    with backpressure pulls_from(producer_rate)
    "Watch {topic}"
```

`pulls_from(name)` is stricter than any bounded buffer and is source-sensitive
when used as a constraint. The VM implements it as a capacity-1 bounded channel,
which forces producers to wait for downstream consumption instead of filling an
unbounded queue.

Fan-in composes upstream policies: any unbounded input makes the merged stream
unbounded; matching pull sources stay pull-based; mixed pull and bounded
sources degrade to the bounded policy because buffering exists somewhere in the
path.

## 21-inv-H-grounded-receipt-narratives

The PR behavior receipt's optional prose summary is now grounded before the
deterministic reviewer can render it:

```corvid
agent review_pr(
    base: Descriptor,
    head: Descriptor,
    impact: TraceImpact,
    narrative_grounded: Grounded<ReceiptNarrative>,
) -> String:
    narrative = narrative_grounded.unwrap_discarding_sources()
    ...
```

Rust still validates that every LLM citation references a real compiler-derived
delta key. Only after validation does the host mint `Grounded<ReceiptNarrative>`
with one provenance entry per cited delta. This preserves the separation of
responsibility: the prompt may write prose, Rust proves the citations are real,
and Corvid's reviewer refuses to consume the narrative as plain ungrounded data.

The CLI executes this embedded reviewer on an explicit worker-thread stack.
That is product behavior, not a test hack: Windows release binaries can have a
smaller main-thread stack than Rust test threads, and complex receipts should
not depend on that platform default.

## 21-inv-H-custom-policy

Trace-diff policy is now governance-as-code in Corvid:

```corvid
@deterministic
agent apply_policy(receipt: PolicyReceipt) -> Verdict:
    ...
```

The key design choice is typed policy facts. Rust converts each canonical
delta key into a `PolicyDelta` with `category`, `operation`, `subject`,
`direction`, `safety_class`, `from_value`, and `to_value`. Policy authors
therefore reason over safety metadata directly instead of parsing strings.

The baked `default_policy.cor` preserves the old conservative gate, but
`--policy=<path>` lets a project replace the gate with its own Corvid program.
The receipt stays archival: custom policies change the verdict, not the
underlying delta record.

`List<T> + List<T>` is now supported so deterministic policy code can build
verdict flag lists naturally.

## 21-inv-H-stacked-aggregate-policy

Stacked PR receipts now gate on history, not just normal form. This matters
because algebraic cancellation can hide a transient safety regression:

```text
commit 1: agent.dangerous_gained:refund_bot
commit 2: agent.dangerous_lost:refund_bot
normal_form: []
history: [dangerous_gained, dangerous_lost]
verdict: failed by default policy
```

That is the right behavior for governance. The final diff may be clean, but the
reviewer still needs to see that the stack carried a dangerous state at an
intermediate waypoint.

The same Corvid policy engine now evaluates both single-commit receipts and
stack receipts. Stack mode passes history-derived `PolicyDelta` facts into
`apply_policy`, and custom `--policy` files can override the verdict without
mutating the archived stack history.

## 21-inv-H-watch-mode

`--format=watch` turns trace-diff into a local development loop:

```text
corvid trace-diff <base-sha> <base-sha> agent.cor --format=watch
```

The first SHA is the stable base. The watched file on disk becomes the live head
on each render. That keeps the mental model simple: CI emits durable receipts;
watch mode gives immediate safety feedback while editing.

The important design constraint is that watch mode reuses the real receipt
pipeline instead of inventing a looser preview path. It still compiles both
versions, computes the same semantic delta, applies the same Corvid policy
engine, and shows the policy verdict. Custom `--policy` files therefore behave
the same locally and in automation.

Watch mode intentionally does not sign receipts or compose stacked reviews.
Those are artifact concerns. The local loop is optimized for speed and clarity;
durable governance still belongs to JSON / in-toto / signed receipt modes.

## 20g-preserved-semantics-rewrite-reports

`corvid test rewrites` is now the public entry point for preserved-semantics
fuzzing. It runs the rewrite coverage matrix over the clean corpus and names
the semantic law attached to each rewrite.

The important part is the failure shape. A drift is not reported as "test
failed"; it names the exact rewrite rule and algebraic law that broke, includes
the first changed source line, shows the original and rewritten effect
profiles, and includes a shrunk reproducer when line deletion can minimize the
case.

Coverage gaps remain honest. Rows with no exercised corpus program are shown as
unexercised, but they are not treated as profile drift. The command distinguishes
"we need a better corpus" from "the checker is unsound under this rewrite."

## 20g-rule-to-test-cross-links

The effect-system spec now has a rule-to-test map. Each row starts from a
language rule family, points at the production module that implements it, then
points at the property/regression tests and corpus gate that keep it honest.

This matters because "inventive" language features need auditability. A reader
should not have to trust a prose claim that `Grounded<T>` or approve-before-
dangerous is enforced; the spec links directly to the checker, VM/runtime test,
and differential-verification gate that prove it.

`corvid test rewrites` is also part of CI now, so preserved-semantics fuzzing is
not an optional local ritual. If an AST rewrite causes profile drift, CI fails
with the rewrite rule, semantic law, first changed line, and shrunk reproducer.

## 20g-counterexample-metadata

The counterexample museum now has explicit metadata in each seed fixture:
counterexample name, bug exposed, fix/proof mechanism, and credit.

This is intentionally small but important. A counterexample without provenance
is just a test file; a counterexample with bug/fix/credit metadata becomes an
auditable safety record. The seed corpus is credited to the Corvid core team
until the public bounty process can attach reporter names to future fixtures.

## 20h-roadmap-reconciliation

The typed model substrate was more complete than the checklist showed. The
spec already has a shipped-section with commit trail, so the roadmap now marks
the real shipped surface complete: `model` declarations, model references,
capability `requires:`, route guards, progressive escalation, rollout,
majority ensembles, adversarial prompt pipelines, jurisdiction/compliance/
privacy dimensions, routing reports, and BYOM adapters.

The important discipline is precision. Some original design bullets changed
shape before shipping: classifier prompts became arbitrary Bool route guards,
`try ... else` became `progressive:`, and generator/validator critic syntax
became a typed three-stage adversarial prompt contract. Those are not
shortcuts; they are stricter language designs. Items that truly did not ship
remain open.

## 20h-cacheable-prompts

Prompt caching is language semantics, not an adapter convenience. The prompt
declares `cacheable: true`, IR carries that bit, and runtime computes a stable
fingerprint from the full semantic call boundary.

Replay determinism is the hard part. A cache hit cannot silently skip trace
events, because then replay would depend on local cache state. Corvid records
cache hits as metadata while still emitting the normal LLM call/result pair,
so replay sees the same behavioral trace and metadata consumers can still
distinguish live calls from cache hits.

## 20h-model-version-pinning

Model names are not stable enough for replay safety. A provider can change the
weights or behavior behind the same name, so Corvid traces now carry an
optional `model_version` and replay treats version drift as semantic drift.

The important product rule is backwards compatibility without silent
weakening. Old traces deserialize with `model_version: null`; new pinned traces
fail fast when the runtime catalog points the same name at a different
version. Reports also use `model@version` labels so operators can see which
revision produced which cost and latency behavior.

## 20h-output-format-routing

Structured output is a model capability, not prompt prose. A prompt requiring
`output_format: strict_json` should not silently route to a model that only
declares markdown-style output, even if that model is cheaper or the default.

The language now carries this constraint through the full stack: source model
catalogs, compile-time validation of named routes, runtime catalog selection,
and trace evidence. That matters because AI-native programs need to prove
compatibility at the boundary where free-form generation becomes typed program
data.

## 20h-weighted-ensemble-routing

Majority vote is a useful baseline, but AI-native routing should learn from
observed behavior. `weighted_by accuracy_history` turns calibration history
into a first-class dispatch signal: a historically reliable minority model can
beat an unreliable raw majority.

Disagreement escalation is the safety valve. When ensemble members disagree,
Corvid can route the same prompt to a declared stronger model instead of
pretending a split vote is decisive. The important design point is that both
the weighting and fallback are visible in syntax, typechecked as model
references, and recorded in traces.

## 20h-eval-swap-model

Retrospective model migration should be trace-based before it is source-eval
based. Production traces already contain the prompts, tools, approvals, costs,
and recorded outcomes needed to answer "what changes if this model changes?"
without re-running unrelated workflow steps.

The implementation discipline matters: `corvid eval --swap-model` is not a
fake eval runner. It delegates to deterministic replay for single traces and to
prod-as-test-suite replay for trace directories, then reports semantic drift
against the candidate model. The full source-level eval runner remains Phase
27, but model migration is useful now.

## 20h-cost-frontier

Cost-quality frontiers are only meaningful when both axes are real. Corvid
already records model-selection cost estimates, but quality must come from eval
evidence rather than routing frequency or confidence folklore.

The command therefore treats missing quality as data absence, not as zero or
average quality. `corvid cost-frontier` computes Pareto status only for models
with explicit eval-quality host events and leaves the rest unscored. That keeps
the operator tooling honest while still making model selection a visible
design-space exploration problem.

## lang-cor-imports-use

Selective imports need a distinct semantic identity. Treating `use Name` as a
local declaration would erase the module boundary; treating it as a plain import
alias would break unqualified calls. Corvid now uses `DeclKind::ImportedUse` so
the compiler knows a lifted name is unqualified at the source level but still
owned by another module.

That distinction matters for future AI-native imports. Effect-typed imports,
hash-pinned imports, and semantic summaries all need to know where a lifted
name came from. The language convenience therefore preserves provenance instead
of hiding it behind wildcard-style namespace merging.

## lang-cor-imports-requires

Imports should carry behavioral contracts, not just names. A policy library
that was deterministic yesterday should not silently become prompt-backed
tomorrow while callers continue compiling as if nothing changed.

Corvid now lets the importing file state the boundary requirement directly:
`import "./policy" requires @deterministic as p` and
`requires @budget(...)` are checked while the import graph is compiled. The
important design choice is that this is not a separate trust system; it reuses
the existing agent attributes and dimensional effect algebra, so module
boundaries participate in the same safety model as local calls.

## lang-cor-imports-semantic-summaries

A module boundary is only useful if developers can inspect what crosses it.
Corvid imports now carry a semantic summary with the public effects, approval
requirements, groundedness, budget cost, and replayability facts that matter to
AI-native code review.

The compiler and CLI read the same summary object. That avoids a common
tooling failure mode where enforcement says one thing but reports show another.
It also sets up signed and remote imports: the thing to hash, sign, diff, and
audit is not just bytes, but the exported semantic contract those bytes imply.

## lang-cor-imports-signed

Supply-chain safety starts before package registries. A local policy path can
drift just as dangerously as a registry package, so Corvid imports now let the
source file pin the imported bytes with `hash:sha256:<digest>`.

The important invariant is fail-closed ordering. The driver verifies the exact
file bytes before lexing, parsing, resolving, or typechecking the imported
module. If the digest changed, the module never enters the compiler's alias map.
That keeps hash pins as a real language trust boundary rather than a comment the
tooling happens to check later.

## lang-cor-imports-remote

Remote imports are only safe if identity is content-addressed. Corvid therefore
rejects `import "https://..." as p` unless the import also carries a SHA-256
pin. The URL says where to fetch; the hash says what program is trusted.

The implementation keeps remote modules in the same semantic pipeline as local
modules instead of inventing a second "package" path. Remote bytes are fetched,
verified, parsed, resolved, summarized, typechecked, and lowered through the
same import machinery. The only special part is module identity: remote files
use deterministic synthetic keys because they do not have filesystem paths.

## lang-cor-imports-versioned-lock

Package imports need two identities, not one. The source identity is semantic:
`corvid://@scope/name/v1.2` is what a developer means. The execution identity is
content-addressed: URL plus SHA-256 is what the compiler can safely trust.

Corvid now keeps those identities separate. Source imports stay stable and
human-readable, while `Corvid.lock` supplies the reviewed URL and digest. This
prevents both shortcut failure modes: source files do not become a pile of raw
hashes, and package imports do not float on mutable registry state.

## lang-cor-imports-versioned-registry

A package manager for an AI-native language cannot be only a downloader. It has
to resolve code and the behavioral contract that code exports. `corvid add`
therefore computes and stores the package semantic summary while writing the
lockfile.

The useful invention is policy-at-install time. Teams can reject packages whose
public exports require approval, violate their own effect constraints, or miss
determinism/replayability requirements before the dependency enters the project.
That makes package resolution part of Corvid's safety model instead of an
external supply-chain step.

## lang-cor-imports-versioned-signed-publish

Signing package bytes is not enough for Corvid. The thing downstream users trust
is bytes plus the AI-safety contract those bytes export. The package signature
therefore covers both the source digest and the computed semantic summary.

That makes signed publish a compiler-facing workflow rather than a registry
decoration. If a package source, URL, version, or exported effect/provenance
surface changes without the publisher re-signing it, `corvid add` rejects it
before the dependency enters `Corvid.lock`.

## proof-carrying custom dimensions

Custom dimensions now have two verification layers. Every dimension still runs
through Corvid's archetype law-check harness, and any dimension that declares a
machine-checkable proof also replays that proof through the relevant assistant:
`.lean` via Lean, `.v` via Coq.

This matters because domain teams can extend Corvid's effect system without
asking the compiler team to hard-code their dimension. The compiler accepts the
extension only if the algebra is executable: property tests pass, and declared
formal proofs actually replay on the local toolchain.

## native shadow replay daemon

Shadow replay is only credible if the daemon can exercise the same tier that
served production traffic. Corvid now makes that tier an explicit daemon
contract: interpreter traces replay under the interpreter executor, and native
traces replay under the native executor selected with `execution_tier = "native"`.

The important invariant is no cross-tier pretending. Native parity is not an
adapter that "mostly" compares native output to interpreter output; it runs the
compiled binary, records a native shadow trace, and rejects traces whose writer
does not match the selected executor. That gives the daemon real deployment
coverage without weakening replay determinism.

## wasm scalar foundation

The browser target has to start from an honest ABI boundary. Corvid can now emit
valid WASM for scalar, runtime-free agents, with JS and TypeScript companions,
but it refuses prompts, tools, and approvals until those capabilities are real
host imports.

That refusal is part of the feature. A WASM target that silently erases approval
or replay semantics would make Corvid less safe than the glue libraries it is
meant to replace. The foundation proves deployment mechanics first and leaves
AI-native host capabilities as the next explicit slice.

## wasm host capability imports

Prompt, tool, and approval calls in WASM are now imports, not erased runtime
magic. The browser host has to provide `prompt.*`, `tool.*`, and
`approve.*` functions, and the generated TypeScript file names the expected
surface.

That design keeps Corvid general-purpose while preserving its AI-native
contracts. A scalar pricing function can compile to standalone WASM; an agent
that calls an LLM compiles to WASM plus a visible host capability requirement.
The next hard part is making those host calls write the same replay traces as
native and interpreter runs.

## wasm trace recording

The generated WASM loader now treats host calls as traceable events. That is the
difference between "Corvid can run in a browser" and "Corvid's replay contract
survives in a browser." Prompt, tool, approval, and run-boundary events use the
same schema names as the interpreter and native tiers.

The remaining gap is execution harnessing: the browser can record compatible
events, but `corvid replay` does not yet drive a WASM module through a
Wasmtime/Wasmer host. That distinction matters because trace shape compatibility
is necessary but not sufficient for deterministic replay.

## wasm browser demo

A browser demo only proves deployment if it imports the generated loader and the
generated WASM module. `examples/wasm_browser_demo` keeps that invariant: the
source is Corvid, the artifacts come from `corvid build --target=wasm`, and the
page supplies the typed host object that the generated `.d.ts` describes.

The honest browser boundary is currently scalar AI-native host capabilities:
prompt, approval, dangerous tool, and trace recording. Strings, structs,
provenance handles, and streaming callbacks are still compiler/runtime work, not
demo-only shortcuts.

## wasm wasmtime parity

WASM parity needs a real runtime in the loop. The Wasmtime harness catches
problems that byte validation cannot: export signatures, host import names,
integer results, branch behavior, and dangerous-action approval flow all have to
work after instantiation.

The harness should follow the WASM ABI boundary, not overclaim beyond it. Today
that means interpreter parity for scalar arithmetic/branching/agent calls and
typed host execution for scalar prompt/approval/tool imports. Full native-corpus
coverage waits for the remaining WASM ABI work.

## lsp diagnostics

The LSP should not reimplement the compiler. Live diagnostics now reuse
`corvid-driver`, which means editor errors match CLI errors for syntax,
resolution, type, approval, effect, provenance, and budget violations.

LSP position mapping has to count UTF-16 code units, not bytes or Unicode scalar
values. Getting this right in a dedicated `position.rs` module prevents hover,
completion, and rename from each inventing their own slightly wrong range math.

## lsp server

The LSP transport should be boring and isolated. `transport.rs` only reads and
writes Content-Length framed JSON-RPC; it does not know how Corvid compiles.
That makes it safe to add hover, completion, and navigation without touching
stdin/stdout framing.

Full-document sync is the correct first server mode. Incremental sync is an
optimization; using full sync keeps live diagnostics correct while the language
surface is still expanding quickly.

## lsp hover

Hover is where Corvid's AI-native semantics become visible while writing code.
It should be compiler-backed for the same reason diagnostics are: effect rows,
approval boundaries, model routes, calibration, grounding, and inferred types
are compiler facts, not editor heuristics.

The initial hover implementation deliberately separates source facts from
protocol transport. `hover.rs` owns semantic summaries; `server.rs` only
serializes the hover response.

## lsp completion

Completion should be context-aware without becoming magical. Approval labels,
effect names, and model names are semantic completions tied to Corvid's
AI-native safety model, while ordinary declarations and keywords keep the
language usable for general programming.

The completion engine should tolerate partial source. Editors ask for
completion while code is incomplete, so `completion.rs` uses the parser's best
available file and narrows by local text context instead of requiring a clean
typecheck.

## lsp navigation

Navigation and rename must use compiler identity, not spelling. A Corvid file
can contain a tool `id` and a parameter `id`; renaming the parameter must not
touch the tool. The resolver already knows this through `DefId` and `LocalId`,
so the LSP should build on that side table.

Single-file navigation is the correct first layer. It gives users definition,
references, rename, and workspace symbols for open documents now, while leaving
cross-file package indexing as a separate package-manager/workspace problem.

## vscode client

The reference editor client should be thin but real. VS Code should not
reimplement diagnostics, hover, completion, or navigation; it should start the
same `corvid-lsp` binary every editor can use and add editor-specific polish:
language registration, highlighting, snippets, restart, and logs.

Server discovery matters for contributors. Supporting explicit setting,
environment variable, repository-local debug/release binaries, and PATH lets the
same extension work in development, installed-tool, and packaged workflows
without hardcoding one layout.

## package manifest lifecycle

Package management has to keep the semantic manifest and immutable lockfile in
sync. `corvid.toml [dependencies]` records the human intent and version
requirement; `Corvid.lock` records the concrete bytes, digest, signature, and
semantic summary. Treating either file as optional creates drift.

Update must reuse add's validation path. A package refresh is still a supply
chain event, so it must re-run source hash verification, signature verification,
semantic-summary extraction, and project policy checks before changing the
lockfile.

## package registry contract

The package registry should be dumb infrastructure. If a registry can be static
`index.toml` plus immutable `.cor` artifacts, the hard security logic stays in
the Corvid client: hash verification, signature verification, semantic-summary
reconstruction, and policy gates.

CDN cache headers are part of the contract. Versioned artifact URLs should be
immutable; if a registry cannot serve `Cache-Control: ... immutable`, users
cannot tell whether a URL is content-stable without relying on trust in the
server.

## package metadata pages

Package pages should be compiler output. A Corvid package is valuable because
of the behavioral contract it exports: effects, approval requirements,
grounding, replayability, determinism, and costs. If those are copied into a
README by hand, they drift. Rendering them from the semantic summary keeps the
registry honest by construction.

Signature provenance is different from source semantics. A local source file can
prove its exported contract, but it cannot prove who published it unless the
registry or publish path supplies a signature. The metadata command therefore
accepts signature provenance explicitly instead of inventing it.

## package conflict verification

Package compatibility is not only semver. In Corvid, the effect contract is part
of dependency compatibility. A package locked yesterday can become invalid today
if the project tightens `[package-policy]` to require replayability,
determinism, signatures, or no approval-required exports.

The right place to enforce that is lockfile verification. `corvid add` prevents
bad new packages from entering the graph; `corvid package verify-lock` proves
the graph remains valid after policy edits, merge conflicts, manual lockfile
changes, or dependency updates.

## test declarations

Testing should reuse the same behavioral assertion language as evals. Corvid
already has trace-aware assertions for process properties; splitting tests and
evals into two assertion models would make one weaker than the other.

The distinction should be runner semantics, not compiler semantics. `test`
declarations are deterministic developer checks; `eval` declarations add
statistical LLM behavior and model-quality reporting. Both can share the same
AST/IR assertion shape.

## test runner

Test execution belongs in the VM, not the CLI. A language-level test can call
agents, prompts, tools, and approvals, so the runner must use the same
interpreter semantics as normal program execution. The driver should compile
and render reports; the CLI should only route arguments and set the process
exit code.

Unsupported AI-native assertions must fail loudly. If `assert called tool` is
preserved by the compiler but the current runner cannot yet inspect traces, a
passing result would be a false safety signal. Reporting an unsupported failure
is less convenient, but it protects the guarantee.

## test mocks and fixtures

Mocks and fixtures should be language declarations, not runner-side text
rewrites. Once they lower into IR, the same resolver, typechecker, LSP,
differential rewrite, and VM paths can validate them.

Mocking must not erase safety boundaries. A mocked dangerous tool still has the
target tool's approval requirement because the interpreter checks the normal
tool gate before substituting the mock body. This keeps tests convenient without
creating a second, weaker execution model.

## test snapshots

Snapshot testing should capture typed runtime values, not rendered source text.
If `assert_snapshot` evaluates through the VM and serializes through the same
value-to-JSON path used elsewhere, snapshots become a stable contract over
program behavior instead of a brittle runner convention.

First-run creation is useful, but silent rewrite is dangerous. Missing
snapshots can be created in normal mode because there is no prior contract to
compare against. Existing mismatches require explicit update mode
(`--update-snapshots` or `CORVID_UPDATE_SNAPSHOTS=1`) so CI failures cannot
accidentally bless behavioral drift.

## trace fixture tests

Process assertions need evidence. `assert called tool` and
`assert approved Label` should not pass because the compiler preserved the
syntax; they should pass only when a trace shows the process happened. Binding
tests to JSONL fixtures gives deterministic CI checks over real production
behavior without re-running LLM calls.

Trace-fixture paths belong in the language declaration, while path resolution
belongs in the driver. The VM should evaluate already-lowered tests and inspect
schema-validated events; it should not guess where the user's source file lives.

## Contributing / feedback

See [CONTRIBUTING.md](CONTRIBUTING.md). The rules of the road are: design chat before code, per-scope commits at every boundary, dev-log entry for every session, no shortcuts. The `learnings.md` file you're reading gets updated when each user-visible feature ships.
