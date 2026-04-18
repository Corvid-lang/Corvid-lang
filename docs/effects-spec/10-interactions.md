# 10 — FFI, generics, async interactions

How the dimensional effect system composes across language and abstraction boundaries.

## 1. FFI — Python tool imports

Corvid programs call Python tools via `import python`:

```corvid
# expect: skip
import python "my_tools" as tools

tool http_fetch(url: String) -> String uses http_effect
```

The Corvid side declares the tool's effect row; the Python side implements the tool body. The checker trusts the declaration — there's no way to statically inspect the Python body's actual behavior.

**Trust boundary.** The effect declaration is the *contract*. If the Python body does something the declaration doesn't mention (e.g. writes to disk when the declaration says `data: none`), the contract is violated and runtime behavior is undefined. This is analogous to Rust's `extern "C"` — the declaration is unchecked against the implementation.

**Recommended practice.** Keep FFI surfaces narrow. Declare tool effects conservatively (over-approximate) so the checker catches misuse even when the Python implementation understates what it does.

## 2. FFI — Rust tool imports

Rust-implemented tools link through the runtime's inventory registration:

```rust
// Rust side
#[corvid_macros::tool]
fn http_fetch(url: &str) -> Result<String, String> { ... }
```

The `#[tool]` macro compiles a tool signature Corvid can call. The Rust side may declare effects via macro arguments (planned for 20i). Today, Rust tools declare effects on the Corvid side and the runtime trusts the binding.

## 3. Generics — `Result<T, E>`, `Option<T>`, `List<T>`, `Grounded<T>`

Generic types preserve effects at composition sites. A tool returning `Result<T, E>` composes its effect row with the caller's as usual; the `Result` wrapper is semantically transparent to the row.

```corvid
# expect: skip
tool maybe_fail(id: String) -> Result<Receipt, Error> uses risky

agent handle(id: String) -> Result<Receipt, Error>:
    result = maybe_fail(id)
    return result
```

- `maybe_fail`'s row is `row(risky)`.
- `result`'s type is `Result<Receipt, Error>`, row is `row(risky)`.
- `return result` propagates row + type. `handle` has row `row(risky)`.

The `?` propagation operator preserves the row:

```corvid
# expect: skip
agent handle(id: String) -> Result<Receipt, Error>:
    result = maybe_fail(id)?     # propagate Err, keep row
    return process(result)
```

## 4. `Grounded<T>` generic interaction

`Grounded<T>` is a *semantic* generic — the wrapper carries a runtime provenance chain and a compile-time obligation (§5 of [03](./03-typing-rules.md)). Composition with other generics is straightforward:

- `Result<Grounded<T>, E>` — either a grounded T or an error. The `?` operator on such a value propagates the error and preserves the grounded chain.
- `Option<Grounded<T>>` — optional grounded value. Unwrap preserves the chain.
- `List<Grounded<T>>` — list of grounded values, each with its own chain.
- `Grounded<List<T>>` — a grounded list (the whole list traces to retrieval). Distinct from `List<Grounded<T>>`.
- `Grounded<Grounded<T>>` — collapses to `Grounded<T>` (chains merge).

The checker permits all of these; the runtime's chain operations distribute correctly.

## 5. Generics and effect declarations

User-defined generic tools are not yet supported. The current type system permits only built-in generics. Adding user-defined generics to declarations (e.g., `tool map<T, U>(xs: List<T>, f: (T) -> U) -> List<U> uses ?`) is a language-level feature tracked in ROADMAP Phase 22.

When this lands, effect rows on generic tools will either be:
- **Parametric**: the caller supplies the effect row for the function parameter.
- **Fixed**: the tool declares its own row regardless of type parameter.

Parametric effects resemble Koka's row polymorphism but bound to the dimensional system. The feature is research-stage; see [11 — Related work](./11-related-work.md) for precedent.

## 6. Async

Corvid's runtime is already async (`tokio` under the hood). Programs that issue concurrent work via future constructs (`spawn`, `join`) will need the effect system to handle concurrency composition:

- **Sequential composition** (current) — effects compose via `⊕` per [02](./02-composition-algebra.md).
- **Parallel composition** (future) — two tasks run concurrently. Per dimension:
  - `cost` — Sum (both are paid).
  - `tokens` — Sum.
  - `latency_ms` — Max (parallelism hides latency up to the slowest task).
  - `trust` — Max (still the most restrictive).
  - `reversible` — AND (still conservative).
  - `data` — Union.
  - `confidence` — Min (still weakest-link).

The composition archetypes already get this right — the only new work is a syntactic form (`parallel { a; b }` or similar) that dispatches to parallel combinators instead of sequential ones. Tracked in Phase 22.

## 7. Effects across Python process boundaries

When Corvid hosts a Python tool and the tool makes its own outbound calls (to APIs, databases, etc.), those downstream effects are *not* visible to the checker. The tool's declared row is the only contract.

Users who need fine-grained tracking across the boundary have two options today:
1. **Re-declare outbound effects in Corvid.** Decompose the Python tool into a Corvid-level chain where each Corvid step wraps one outbound API.
2. **Runtime instrumentation.** The trace file captures every call the runtime observes, so `corvid verify` and `corvid effect-diff` report on observed behavior even when the static checker can't prove it.

A fully traced FFI with declared inner-effect boundaries is planned as a Phase 23 extension.

## 8. Weak references and effect interaction

Corvid's `Weak<T>` type interacts with the effect system through the `weak` effect row. See the main language reference §17; the short version is that `Weak::upgrade` requires a proof that no invalidating effect occurred along every path from the most recent `Weak::new` or `Weak::upgrade`. This is a separate typing judgment from dimensional composition, but it uses the same effect-row machinery.

## 9. Effect-system extensions at FFI boundaries

The custom-dimension mechanism ([01 § 4](./01-dimensional-syntax.md)) means FFI boundaries can declare custom dimensions that the host application cares about — `jurisdiction`, `data_retention_days`, `pii_detected`, etc. The checker treats them uniformly. For Python tools, the declared custom-dimension values on the Corvid side are the contract; the Python implementation must respect them.

## Next

[11 — Related work](./11-related-work.md) — Koka, Eff, Frank, Haskell monad transformers, Rust `unsafe`, capability security, linear types, session types.
