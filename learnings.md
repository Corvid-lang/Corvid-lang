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

## Contributing / feedback

See [CONTRIBUTING.md](CONTRIBUTING.md). The rules of the road are: design chat before code, per-scope commits at every boundary, dev-log entry for every session, no shortcuts. The `learnings.md` file you're reading gets updated when each user-visible feature ships.
