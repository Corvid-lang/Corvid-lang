# Corvid — Features & Roadmap

> The feature roadmap from v0.1 through v1.0. Every feature on this list has earned its place. Adding a feature requires justification in `dev-log.md`; removing one requires archiving the rationale.
>
> See also: [`ROADMAP.md`](./ROADMAP.md) for the phase-by-phase build plan.

---

## v1.0 product definition

**What users will download at v1.0:**

- A single native binary (`corvid`).
- The compiler emits native machine code via Cranelift.
- Generated programs link a Rust-native runtime and run without any Python.
- Python FFI is available via `import python "..."`, loaded lazily only when used.

v1.0 is not an "evolved v0.1." The Python-transpile backend we shipped internally in v0.1 stays as `--target=python` for users who want it, but the default, the marketing, and the "this is Corvid" experience is the standalone native compiler.

---

## Guiding rule

For every feature, ask: **"If I remove this, does the language still have a reason to exist?"**

- Remove any v0.1 feature → the pitch dies.
- Remove any v0.2 feature → the language is unusable in real projects.
- Remove any v0.3 feature → the moat disappears.
- v0.4+ features are nice-to-have; they must not delay earlier releases.

---

## v0.1 — Foundation ✅ COMPLETE (Python transpile backend)

Internal milestone. Full frontend + Python codegen. All in.

1. Typed prompts as first-class declarations
2. Tools with `dangerous` effect annotation
3. Compiler-enforced approval for dangerous effects
4. Agents as top-level declarations
5. Structured output with typed returns
6. Python codegen (opt-in at v1.0 via `--target=python`)
7. CLI (`corvid new`, `check`, `build`, `run`, `doctor`)
8. Ariadne-quality error messages with stable error codes

Status: 134 Rust + 10 Python tests green. Canonical `refund_bot.cor` compiles, runs, enforces approve-before-dangerous at compile time.

---

## v0.2 — Standalone interpreter (in progress)

Replaces the Python-shell-out runtime with a native Rust runtime. Users still don't see this publicly — it's the scaffolding v1.0 is built on.

1. **Tree-walking interpreter** — `corvid-vm` crate walks the IR, executes directly.
2. **Native HTTP + Anthropic adapter** — `reqwest` + JSON, no Python needed for LLM calls.
3. **Native tool registry** — tools registered from Rust; `.cor`-defined tools arrive in v0.3.
4. **Native approval flow** — stdin prompt + programmatic hook, all in Rust.
5. **Native tracing** — JSONL writer in Rust.
6. **CLI: `corvid run` executes natively** — Python is no longer on the critical path.

Output: users running `corvid run refund_bot.cor` don't need Python installed. Python codegen remains for `--target=python`.

---

## v0.3 — Language-feature parity + durability

Features that turn triers into production users.

1. **Testing primitives** — `test`, `mock`, `fixture` as language features.
2. **Multi-provider LLM** — OpenAI, Google, Ollama adapters alongside Anthropic.
3. **Memory primitives** — `session`, `memory` as typed, SQLite-backed stores.
4. **Error handling** — typed `Result` / `Option`; retry policies as syntax.
5. **HITL beyond `approve`** — `ask(...)` for clarifications, `choose(...)` for options.
6. **Python FFI via PyO3, effect-tagged at the import site** — `import python "lib" as x effects: network, spends` works from the interpreter. Untagged Python imports rejected by default; `effects: unsafe` exists as an opt-in escape hatch and is visually discouraged. TypeScript's `.d.ts` analog: the compiler trusts declared effects, so untagged Python usage cannot be introduced by accident.

---

## v0.4 — Differentiation (the moat)

Hard-to-copy features that a compiler enables and a library can't.

1. **Uncertainty types** — `T?confidence`; compiler forces low-confidence handling.
2. **Cost budgets** — compile-time-checked spend caps.
3. **Streaming as first-class type** — `Stream<Token>`, `Stream<T>` for partial structured outputs.
4. **Prompt-aware compilation** — prompt deduplication, schema caching, TOON-compressed payloads.
5. **Replay as a language concept** — every run replayable by construction; `corvid replay` as a primitive.
6. **Custom effects + effect rows** — user-declared `effect Name` definitions beyond `safe`/`dangerous` (`retrieves`, `spends`, `reads_pii`, `mutates_db`, `cites`, etc.). Effect rows on tool/agent signatures; compiler verifies body against declaration. Data-flow tracking: callers inherit callees' effects unless explicitly scoped. Per-effect approval policy declarable. Property-based test suite proves the checker cannot be bypassed via FFI, generics, or indirect calls.
7. **Grounding + citation contracts** — compile-time RAG correctness.
   - `grounds_on ctx` annotation on prompts; template must reference `ctx` or compile error `E0201`.
   - `cites ctx` effect on prompts; return type must be `Grounded<T>` or `E0202`; template must request citations or `E0203`.
   - `cites ctx strictly` opts into runtime citation-verification failure.
   - `Grounded<T>` is a compiler-known stdlib type; unwrapping requires `.unwrap_discarding_sources()`.
   - The vector store, document loaders, chunking, and embedder ship as the separate `corvid-rag` package — library, not language.
8. **Eval as language syntax** — first-class `eval <agent> on dataset(...) { assert ... }` declarations with typed assertion predicates (approval_rate, grounding_rate, citation_verification_rate, average_cost, average_latency). The `corvid eval` CLI command, HTML reports, and CI integration land in v0.7.
9. **Written effect-system specification** — 20–40 page document (syntax, typing rules, worked examples, FFI/async/generics interactions, related-work: Koka, Eff, Frank, Haskell effect libs, Rust `unsafe`, capability systems). Ships at the v0.4 boundary.

---

## v0.5 — Cranelift native backend

The switchover. v1.0's performance story materializes here.

1. **Cranelift integration** — `corvid-codegen-cl` crate, IR → Cranelift IR → native.
2. **Native runtime linkage** — compiled binaries statically embed `corvid-runtime`.
3. **AOT compile path** — `corvid build refund_bot.cor` produces `target/bin/refund_bot`.
4. **Compiler-vs-interpreter parity tests** — every fixture validated on both tiers.
5. **Benchmarks** — non-LLM code within 2× of hand-written Rust for the demo suite.
6. **Async runtime integration** — Tokio linked in; `async` native at the machine-code level.

---

## v0.6 — Scale

Features for serious multi-agent production systems.

1. **Multi-agent composition** — agents calling agents with automatic trace merging.
2. **Durable execution** — crash-safe by default; no Temporal needed.
3. **Observability built in** — tracing, cost analytics, per-agent dashboards.
4. **Policy system** — declarative rate limits, auth, auditing.
5. **Hot reload** — edit agent; in-flight runs keep version; new runs use new code.
6. **WASM target** — `corvid-codegen-wasm` reads the same IR; runs in browsers, Deno, edge.

---

## v0.7 — Ecosystem

What makes a language a movement.

1. **Package manager** — `corvid add <package>`. Study Cargo; copy what works.
2. **IDE support** — LSP server, VS Code extension, inline trace viewer.
3. **Standard library** — common agent patterns (RAG, tool-use, planning) as stdlib.
4. **Eval tooling** — `corvid eval` CLI command, terminal + HTML reports, regression detection against previous runs, CI integration (exit code 0/non-zero). The language syntax (`eval ... assert ...`) already landed in v0.4; this phase adds the runner, the reports, and the CI contract.

---

## v1.0 — Launch

Stable API, documented, production-ready.

1. **Stable language spec** — semver `1.0` guarantees; no breaking changes without major bump.
2. **Full docs + tutorial + book**.
3. **Installer** — `curl -fsSL corvid.dev/install.sh | sh` distributes the binary and sets up the environment.
4. **Public launch** — HN, conferences, keynote demo.
5. **Windows + Linux + macOS** binaries shipped through the installer.

---

## Explicitly deferred past v1.0

Not necessarily bad ideas, but not v1.0 scope:

- Macros / metaprogramming
- Dependent types
- Linear / affine types
- Formal verification hooks
- Distributed agent orchestration
- Multi-model ensemble primitives
- Fine-tuning as a language feature
- Visual/block-based editor
- RAG framework (vector store, loaders, chunking, reranking) bundled in the compiler distribution — the grounding/citation *contracts* are language; the runtime substrate ships as the separate `corvid-rag` package
- MCP runtime client/server — belongs in `corvid-runtime` or a package, not the compiler core (effect tagging for MCP calls can arrive with custom effects in v0.4)
- Typed Python library wrappers (`std.python.anthropic`, etc.) — library work, not language
- `corvid-py` Python-embedding package — distribution/library question, handled outside the language roadmap

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
