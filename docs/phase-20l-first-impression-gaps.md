# Phase 20l — First-Impression Gap Repair

Closes the eight language-side gaps surfaced by an external reviewer
building a non-trivial sample app (a ticket-triage agent exercising
effects, dangerous tools, approve boundaries, prompts, budgets, and
stdlib imports) against the workspace at HEAD post-Phase 20k.

Original gap report — verbatim verbatim — lives in the issue tracker
and at the bottom of this document for reference. Every gap has been
re-verified against the current codebase before being scoped here.

## Why this phase exists

Phase 20j and 20k closed with the workspace responsibility-rubric
clean. The demo pack (33K) and reference-app hardening pass (42H)
landed. The Corvid language platform itself is shipped.

But responsibility-rubric clean and demo-pack-shipped are not the same
thing as first-impression clean. An external reviewer test-drove
Corvid end-to-end on a sample app and surfaced eight rough edges:

- A correctness bug that makes `corvid check` silently OK code that
  won't build (L-1).
- A type-fidelity regression in the Python codegen (L-2).
- Two "currently supports only..." errors in native + WASM codegen
  for non-trivial program shapes (L-3, L-4).
- An unactionable error path when the native staticlib is missing in
  dev environments (L-5, re-diagnosed from the original report).
- ANSI escapes leaking into non-TTY stderr on Windows (L-6).
- Lexer doesn't accept `\` end-of-line continuation (L-7).
- An undocumented `approve`-name PascalCase rule (L-8).

L-1, L-2, L-6, the re-diagnosed L-5, and L-8 are the "should fix this
week" set. L-7 is optional polish. L-3 and L-4 are real feature work
filed as deferrals to their owning phases.

## Sequencing rules

Per CLAUDE.md "When splitting" — unchanged from 20j/20k:

- One commit per fix. No batching.
- Validation gate between every commit:
  - `cargo check --workspace` (zero new errors)
  - `cargo test -p <crate-modified>` (lib + integration tests green)
  - `cargo run -q -p corvid-cli -- verify --corpus tests/corpus`
    (capture exit with `> file 2>&1; echo exit=$?`; baseline is
    exit 2 from the established Windows whoami linker error)
- Push before starting the next slice.
- Pre-phase chat per slice; no autonomous chaining.
- Zero unrelated changes during a fix commit. Each slice ships:
  - The fix
  - A regression test that fails before the fix and passes after
  - A dev-log entry
  - A learnings.md entry if the fix is user-visible
- Commit message: `<type>(<crate>): <imperative summary>` — body
  cites the slice id (20l-A through 20l-F), names the reproduction,
  root cause, and validation commands run.

## Slices

### 20l-A — `corvid check` resolves imports (L-1, Critical)

**Reproduction.** A `.cor` file with `import "./foo" use Bar` where
the imported module either doesn't exist on disk or doesn't export
`Bar`:

```sh
corvid check src/main.cor
# ok: src/main.cor — no errors        <-- LIES
corvid build src/main.cor
# error: import './foo' from '...src/main.cor' could not be found
```

**Verified site.** `crates/corvid-cli/src/commands/misc.rs:40`:

```rust
let result = compile_with_config(&source, config.as_ref());
```

The path-less driver entry can't resolve sibling `.cor` imports —
the driver's own doc comment at `pipeline/compile.rs:91` says so.
But `cmd_check` already has `file: &Path` and isn't passing it.

**Fix.** Switch the import + the call to the path-anchored variant:

```diff
 use corvid_driver::{
-    compile_with_config, diff_snapshots, load_corvid_config_for, render_all_pretty,
+    compile_with_config_at_path, diff_snapshots, load_corvid_config_for, render_all_pretty,
     render_effect_diff, scaffold_new, snapshot_revision, vendor_std,
 };

 pub(crate) fn cmd_check(file: &Path) -> Result<u8> {
     let source = std::fs::read_to_string(file)
         .with_context(|| format!("cannot read `{}`", file.display()))?;
     let config = load_corvid_config_for(file);
-    let result = compile_with_config(&source, config.as_ref());
+    let result = compile_with_config_at_path(&source, file, config.as_ref());
```

Three lines. Both functions return `CompileResult`. No new logic.

**Regression test.** `crates/corvid-cli/tests/check_validates_imports.rs`
shells out to the `corvid` bin against a tempdir program that imports
`./does_not_exist` and asserts `corvid check` exits non-zero with a
"could not be found" diagnostic.

**Acceptance.** `corvid check` rejects the missing-import case;
`corvid check` on a clean program still returns ok; existing tests
remain green.

**Estimated commits:** 1.

---

### 20l-B — Python codegen preserves struct + container types (L-2, High)

**Reproduction.** A Corvid file with a nested struct field:

```corvid
type Severity:
    label: String
    confidence: Float

type Triage:
    severity: Severity
    summary: String
```

Generated `target/py/main.py`:

```python
@dataclass
class Triage:
    severity: object       # <-- should be: "Severity"
    summary: str
```

`mypy` / `pyright` / IDE autocomplete all collapse on `object`.

**Verified site.** `crates/corvid-codegen-py/src/codegen.rs:495–519`,
`python_type_hint_of`:

```rust
T::Struct(_)
| T::ImportedStruct(_)
| T::Function { .. }
| T::List(_)
| T::Stream(_)
| T::Partial(_)
| T::ResumeToken(_)
| T::RouteParams(_)
| T::Unknown => "object".into(),
```

The comment two lines below acknowledges this is a TODO: *"Emitting
'object' here is a safe approximation until the Python backend
decides on its representation."*

**Fix.** Emit forward-ref string literals for struct types and
mechanical recursion for the container types:

```diff
-        T::Struct(_)
-        | T::ImportedStruct(_)
-        | T::Function { .. }
+        T::Struct(name) => format!("\"{}\"", name),
+        T::ImportedStruct(qname) => format!("\"{}\"", qname.local_name()),
+        T::List(inner) => format!("list[{}]", python_type_hint_of(inner)),
+        T::Option(inner) => format!("{} | None", python_type_hint_of(inner)),
+        T::Function { .. }
-        | T::List(_)
         | T::Stream(_)
         | T::Partial(_)
         | T::ResumeToken(_)
         | T::RouteParams(_)
         | T::Unknown => "object".into(),
-        T::Result(_, _) | T::Option(_) | T::Weak(_, _) => "object".into(),
+        T::Result(_, _) | T::Weak(_, _) => "object".into(),
```

The generated module already imports `from __future__ import
annotations` at line 2, so forward-refs as strings are unnecessary
under PEP 563 — but explicit string forward-refs remain forward-
compatible if PEP 563 is ever rolled back. Pick whichever the
codegen owner prefers; both work.

`Stream(_)` / `Partial(_)` / `ResumeToken(_)` / `RouteParams(_)`
stay `object` until they grow real Python representations (separate
slice if needed, not in 20l).

**Regression test.** `crates/corvid-codegen-py/tests/struct_field_types.rs`:

```rust
#[test]
fn nested_struct_field_emits_class_name() {
    let py = corvid_codegen_py::compile_to_python(r#"
        type Inner:
            x: Int
        type Outer:
            inner: Inner
        agent main() -> Outer:
            return Outer(Inner(42))
    "#).unwrap();
    assert!(
        py.contains("inner: \"Inner\"") || py.contains("inner: Inner"),
        "expected struct-name forward-ref, got: {py}"
    );
}
```

Plus equivalent tests for `list[T]` and `T | None` shapes.

**Acceptance.** Generated Python carries struct names, list element
types, and option inner types instead of `object`. `mypy --strict`
on a generated module + a hand-written caller succeeds.

**Estimated commits:** 1–2 (single commit if container recursion is
co-shipped; split if separated).

---

### 20l-C — Diagnostic renderer auto-detects TTY (L-6, Low-medium)

**Reproduction.** PowerShell on Windows (classic conhost):

```sh
corvid check src/main.cor 2>&1
# corvid.exe : [31m[E0003] error:[0m unexpected character `\`
#  [38;5;246m╭[0m...
```

Setting `$env:NO_COLOR=1` cleans it up — so the renderer respects
`NO_COLOR` (via ariadne) but doesn't auto-detect non-TTY.

**Verified site.** `crates/corvid-driver/src/render.rs:28`:

```rust
.with_color(Color::Red),
```

Always emits color. No `is_terminal()` check.

**Fix.** ~5 lines using `std::io::IsTerminal` (Rust ≥1.70, no new
dep):

```diff
+use std::io::IsTerminal;

 pub fn render_pretty(diag: &Diagnostic, source_path: &Path, source: &str) -> String {
-    let kind = ReportKind::Custom("error", Color::Red);
+    let with_color = std::env::var_os("NO_COLOR").is_none()
+        && std::io::stderr().is_terminal();
+    let kind = if with_color {
+        ReportKind::Custom("error", Color::Red)
+    } else {
+        ReportKind::Custom("error", Color::Default)
+    };
```

Plus the matching `with_color` swap on `Label::with_color`.

**Regression test.** Snapshot test that invokes the bin via
`Command` with stderr captured (non-TTY) and asserts the captured
output contains zero `\x1b[` escape sequences.

**Acceptance.** Color emitted only when stderr is a real terminal;
piped / captured / redirected output is plain text.

**Estimated commits:** 1.

---

### 20l-D — Native staticlib-missing diagnostic actionable (L-5 re-diagnosed)

**Reporter's original framing.** "auto-dispatch picks native when
interpreter would suffice."

**Re-diagnosis.** The dispatch is intentional. `crates/corvid-driver/src/run.rs:162`:

```rust
RunTarget::Auto => match &scan {
    Ok(()) => run_via_native_tier(...),     // pure programs → native
    Err(reason) if tools_satisfy(reason) => native...,
    Err(reason) => {
        eprintln!("↻ running via interpreter: {reason}");
        run_via_interpreter_tier(...)
    }
},
```

Plus the explicit test `run_with_target_auto_uses_native_for_pure_program`.
The reporter's suggested fix (flip default to interpreter) would change
semantic intent and break that test.

**Actual gap.** The native path fails on missing
`corvid_runtime.lib` in dev environments with this diagnostic:

```
linker error: corvid-runtime staticlib missing at
'<exe-dir>/corvid_runtime.lib' and no release fallback was found.
```

The error tells the user where it looked but not how to recover.

**Fix.** ~10 lines in `crates/corvid-driver/src/run.rs`: amend the
"no release fallback was found" diagnostic to name the recovery
command:

```
linker error: corvid-runtime staticlib missing at
'<exe-dir>/corvid_runtime.lib'.

To populate the dev path, run:
  cargo build -p corvid-runtime --release

Or force the interpreter for this run:
  corvid run --target=interpreter <file>
```

**Regression test.** Integration test that runs the staticlib-missing
path and asserts the error message contains `cargo build -p
corvid-runtime`.

**Acceptance.** Users hitting the missing-staticlib path on first
`corvid run` see an actionable command instead of a dead-end
diagnostic. No semantic dispatch change.

**Estimated commits:** 1–2.

---

### 20l-E — Document `approve` PascalCase rule (L-8, docs only)

**Verified behaviour.** Compiler correctly rejects `approve PageOnCall(...)`
when the dangerous tool is `send_to_pagerduty`. Diagnostic E0101 names
the right shape. Greppability per-tool is the security property —
`grep '^\s*approve SendToPagerduty\b'` finds every approval site.

**Gap.** The rule isn't documented anywhere a new user will look:

- `docs/effects-spec/03-typing-rules.md` doesn't state the
  PascalCase mapping explicitly.
- `corvid tour --topic approve-gates` shows the syntax without
  naming the rule.
- New users learn the rule from the compiler error, not the docs.

**Fix.** Add a section to the typing rules spec titled "approve
identifier naming":

> The identifier following `approve` must be the PascalCase form of
> the dangerous tool's snake_case name. The compiler rejects
> mismatches at typecheck time with `E0101: dangerous tool 'X'
> called without a prior 'approve'`. This makes approval sites
> greppable per-tool: `grep '^\s*approve TransferFunds\b'` finds
> every authorised call site for `transfer_funds`.

Plus a one-line addition to the `corvid tour --topic approve-gates`
blurb naming the convention.

**Acceptance.** A new user reading the typing spec learns the rule
before writing their first dangerous tool. Tour blurb references
the rule.

**Estimated commits:** 1.

---

### 20l-F — Lexer accepts `\` end-of-line continuation (L-7, Low, optional)

**Reproduction.** Pythonic line continuation between adjacent string
literals:

```corvid
agent main() -> String:
    return "first part " \
           "second part"
# [E0003] error: unexpected character `\`
```

**Verified site.** `crates/corvid-syntax/src/lexer.rs:437` —
top-level `\` outside strings hits the `_ =>` arm and fires
`UnexpectedChar`. No line-continuation handling.

**Fix.** ~10 lines: add a top-level branch for `b'\\'` that, if
followed by `\n` (with optional trailing `\r`), consumes both bytes
and emits no token. If followed by anything else, fall through to
the `UnexpectedChar` path.

**Acceptance.** Adjacent string literals can be continued with `\`
at end-of-line. Existing programs that use `\` inside string
literals via `consume_escape` keep working unchanged.

**Optional.** Workarounds exist (`+` concatenation, triple-quoted
strings). Ship only if 20l-A through 20l-E land cleanly and the
slice gate has room. Otherwise file a learnings entry "deferred —
workarounds suffice; design choice rather than bug" and close 20l
without it.

**Estimated commits:** 1.

---

## Filed as deferrals (not 20l slices)

### L-3 — Native struct returns from prompts

**Component.** `crates/corvid-codegen-cl/src/lowering/prompt.rs:371`.

**Current behaviour.** Honest "not yet implemented" error:

```
prompt 'classify' returns 'struct' — the native prompt bridge currently
supports only Int / Bool / Float / String returns; structured prompt
returns are not implemented yet
```

**What it needs.** The native prompt bridge has to allocate a struct
on the runtime heap, deserialize the LLM JSON response into it via
the existing `corvid-runtime` JSON deserializer, and return a pointer
the way `Grounded<T>` already handles primitives.

**Where filed.** Phase 17 follow-up (cycle collector + memory model)
or Phase 20 (moat — `Grounded<T>`). Both touch the same heap-handle
machinery. Pick whichever phase doc the next session decides to
extend.

**Workaround for users today.** Return a flat tuple of primitives
from the prompt and re-pack into the struct in the calling agent.
Or use `--target=python` until the native path catches up.

### L-4 — WASM `String` parameters

**Component.** `crates/corvid-codegen-wasm/src/lib.rs:157`.

**Current behaviour.** Honest "currently supports only" error:

```
wasm target currently supports only Int, Float, Bool, and Nothing
scalar parameters; agent 'auto_respond' parameter 'ticket' has 'String'
```

**What it needs.** Pick a string ABI (UTF-8 + length is the common
choice; WASM Component Model has a richer story) and thread it
through codegen + the JS loader. Real engineering work.

**Where filed.** Phase 23 follow-up (WASM target). The Phase 23
heading already carries `(reopened 2026-04-29 — browser end-to-end
CI gap)`; L-4 fits the same audit-correction track.

**Workaround for users today.** Stick to scalar parameters in
WASM-targeted agents. For browser embedding the Python-target via
Pyodide is an alternative until the String ABI lands.

---

## Phase-done checklist

- [ ] 20l-A `corvid check` resolves imports — landed with regression
  test.
- [ ] 20l-B Python codegen forward-refs — landed with regression
  test.
- [ ] 20l-C TTY auto-detect — landed with snapshot test.
- [ ] 20l-D Staticlib-missing diagnostic actionable — landed with
  integration test.
- [ ] 20l-E `approve` PascalCase rule documented in spec + tour
  blurb.
- [ ] 20l-F `\` line continuation — landed OR deferred with
  documented rationale.
- [ ] L-3 filed against Phase 17 or 20 follow-up doc.
- [ ] L-4 filed against Phase 23 audit-correction follow-up doc.
- [ ] Closing audit appended to this document with per-slice shipped
  state.
- [ ] `learnings.md` updated per slice.
- [ ] ROADMAP.md Phase 20l entry checkboxes ticked.
- [ ] Memory record `project_phase_20l_closed.md` summarising the
  recurring "first-impression gap" patterns:
  (a) path-anchored API used in some commands but not others —
      the L-1 shape, watch every `cmd_*` that takes a path
      argument and verify it threads through to the path-anchored
      driver entry;
  (b) codegen TODOs that ship as `object`-shaped degradations —
      the L-2 shape, watch for "safe approximation" comments next
      to type-emission code and verify the eventual emission round-
      trips a name;
  (c) diagnostic surface that didn't auto-detect environment —
      the L-6 shape, watch every renderer/UI surface for
      `is_terminal()` / `NO_COLOR` / explicit verbosity flags.
  Memory file path:
  `C:\Users\SBW\.claude\projects\c--Users-SBW-OneDrive---Axon-Group-Documents-GitHub-corvid\memory\project_phase_20l_closed.md`.
  Add a one-liner to MEMORY.md.

## Sequencing reminder

Per CLAUDE.md "pre-phase chat mandatory" and "no autonomous chaining":
each slice gets its own pre-phase confirmation before any code edits.
Refactor commits land sequentially with push between, never batched.

The recommended order (smallest blast radius first → larger):
**A → C → E → D → B → F**. A and C are mechanical 1-commit fixes that
restore correctness for downstream tooling; E is a docs-only paragraph;
D is a small dispatch error-message rewrite; B is the substantive
codegen patch; F is optional polish.
