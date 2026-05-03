# Phase 20k — Strict Single-Responsibility Pass

Tightens the CLAUDE.md responsibility rubric from "1–2 responsibilities per
file" to **exactly one**, with three explicit carve-outs. Then sweeps the
workspace against the strict rule and decomposes the files that pass under
the loose reading but fail under strict.

## Why this phase exists

Phase 20j (closed 2026-05-03) decomposed every one of the 37 originally-flagged
mixed-domain files. The closing audit confirmed the original responsibility
violations are gone, and ~14 large root modules remain that "do one thing
plus tests" or "facade plus typed records" — they pass the loose 1–2 rule
but hold two cohesive concepts each.

Under a strict 1-responsibility rule, those mod.rs roots either:

- **Split into peer files**, where the typed records get their own
  `records.rs`, the cross-domain test cluster moves to `tests.rs` (or
  splits per domain into the sibling sub-modules), and mod.rs becomes a
  pure facade (or holds only the type-and-its-impls cluster).
- **Stay intact under a carve-out** — when the file genuinely is one
  responsibility plus its own canonical co-located tests, or when it's a
  facade that exists to compose siblings.

This phase formalises which files need which treatment.

## The strict rule (now in CLAUDE.md)

> Every source file holds exactly one responsibility.
>
> A file fails when:
>
> 1. It mixes unrelated top-level concepts (parsing + lexing; checking + IR
>    lowering; dispatch + recording).
> 2. It has 2+ public items representing unrelated domains.
> 3. It has 2+ internal sections that share no state.

### Carve-outs (these still count as one responsibility)

1. **Inline `#[cfg(test)] mod tests`** — co-located unit tests for the
   file's own type/concept are part of that responsibility, not a second
   one. Extract the tests when they grow past ~300 lines OR when they
   cover sibling-module concerns rather than this file's own concept.
2. **A type with its inherent + canonical-derive trait impls** —
   `struct Foo` + `impl Foo` + `impl Clone for Foo` + `impl Drop for Foo`
   + `impl PartialEq for Foo` are one responsibility ("the Foo type").
   Cross-cutting trait impls (e.g. a `Render` trait implemented for ten
   record types) get their own file per impl-cluster.
3. **A facade module** — a thin module that exists to compose siblings is
   one responsibility ("the facade"). Re-exports + a small orchestrator
   struct + a thin dispatcher all count as one concern, even though
   they're three syntactic items.

## Sequencing rules

Per CLAUDE.md "When splitting" — unchanged from 20j:

- One commit per file extraction. No batching.
- Validation gate between every commit: `cargo check --workspace` +
  targeted `cargo test -p <crate> --lib` + `cargo run -q -p corvid-cli
  -- verify --corpus tests/corpus`.
- Push before starting the next extraction.
- Pre-phase chat at every sub-slice. No autonomous chaining.
- Zero semantic changes during a refactor commit. Move code, add `pub
  use` re-exports to preserve the public API, nothing else. Bugs spotted
  mid-refactor get a separate branch.
- Commit message: `refactor(<crate>): extract <responsibility> from
  <file>` — body names which strict-rubric criterion failed and how the
  split resolves it.

## Slices

### 20k-audit — workspace sweep against strict rule

Spawn a `general-purpose` agent to audit every `.rs` file under `crates/`
against the strict rule + carve-outs. Same prompt shape that found the
31 violators in 20j. Output: an inventory table with rubric criterion
failed, mixed concerns, target decomposition, per-extraction commit
plan.

This slice produces the candidate list that drives the rest of 20k.

### 20k-A10c — auth records and tests split (pattern reference)

Already named in 20j's closing audit as deferred. Serves as the pattern
reference for every 20k impl-method-cluster split with cross-domain
tests.

`corvid-runtime/src/auth/mod.rs` is currently 764 lines holding:

- 16 typed records (~200 lines) — the auth domain's data shape
- `pub struct SessionAuthRuntime` + `open` / `open_in_memory` /
  `upsert_actor` / `get_actor` / `init` (DDL) — actor surface
- `pub(super) fn validate_non_empty` — cross-domain helper
- ~370 lines of tests covering all four auth domains (sessions, api
  keys, oauth, audit), not just mod.rs's own actor surface

Under strict rule that's three responsibilities (records + actor
surface + cross-domain tests).

**Proposed split — 5 commits:**

1. `extract records from auth` → `auth/records.rs` holds the 16 typed
   records. mod.rs re-exports via `pub use records::*`.
2. `relocate session_runtime tests to sessions` — the four
   `session_runtime_*` / `session_rotation_*` tests move into
   `sessions.rs`'s `#[cfg(test)] mod tests`.
3. `relocate api_key_runtime tests to api_keys` — the two
   `api_key_runtime_*` tests move into `api_keys.rs`.
4. `relocate oauth tests to oauth` — the three `oauth_*` /
   `jwt_contract_validation_*` / `permission_propagation_*` tests
   move into `oauth.rs` (and `approvals.rs` if any).
5. `collapse auth mod to actor surface` — what remains in mod.rs is
   the actor surface + DDL + module declarations + the
   `validate_non_empty` helper. Target: ~150 lines.

### 20k-* — additional sub-slices defined by the audit step

Filled in by 20k-audit's output. Strong-signal initial guesses pending
verification:

- `queue/mod.rs` (1,527 — ~1,140 lines are tests covering 6 sibling
  domains) — likely splits tests into per-cluster `#[cfg(test)] mod
  tests` in each sibling.
- `runtime/mod.rs` (1,414) — likely a similar tests-split or
  builder/runtime separation.
- `lowering/runtime/mod.rs` (1,431) and `lowering/expr/mod.rs` (1,192)
   — same shape; verify.
- `replay/mod.rs` (924) — replay source + tests; verify.
- `approval_queue.rs` (866) — workflow + tests; verify.
- `interp.rs` (1,056) — dispatch + tests; verify.
- Files outside the original 20j audit and ≥1,000 lines that may now
  be in scope: `corvid-repl/src/lib.rs` (2,345),
  `corvid-differential-verify/src/rewrite.rs` (1,929),
  `corvid-driver/src/modules.rs` (1,455), `corvid-ir/src/lower.rs`
  (1,407), `corvid-runtime/src/catalog_c_api.rs` (1,385),
  `corvid-cli/src/cli/root.rs` (1,369),
  `corvid-cli/src/dispatch.rs` (1,192),
  `corvid-guarantees/src/registry.rs` (1,148),
  `corvid-resolve/src/resolver.rs` (1,042),
  `corvid-differential-verify/src/lib.rs` (1,020),
  `corvid-types/src/checker/decl.rs` (1,010),
  `corvid-ir/src/lib.rs` (1,009).

The audit step decides which of these actually need work and which
are single-concern roots that pass.

## Validation gate

Run between every commit, in order:

```bash
cargo check --workspace
cargo test -p <crate-being-modified> --lib
cargo run -q -p corvid-cli -- verify --corpus tests/corpus
```

Pass criteria:

1. `cargo check --workspace` reports zero new errors.
2. Targeted lib tests pass for every modified crate.
3. `corvid verify --corpus tests/corpus` exit signature matches the
   pre-existing `whoami` Windows linker baseline (exit 2, environmental).

## Phase-done checklist

- [ ] 20k-audit complete; candidate list recorded in this document.
- [ ] 20k-A10c complete (5 commits).
- [ ] All audit-discovered sub-slices complete.
- [ ] Workspace re-audit confirms every `.rs` file in `crates/` passes
  the strict rubric or is a documented carve-out.
- [ ] `docs/phase-20k-refactor.md` updated with closing inventory:
  per-file post-split line counts and target-module list, mirroring
  20j's closing-audit format.
- [ ] `learnings.md` updated per slice.
- [ ] ROADMAP.md's Phase 20k entry is checked.
- [ ] Memory record `project_phase_20k_closed.md` written summarising
  which concept-pairings tend to coexist (records + facade, type +
  cross-domain tests, dispatch + recording) so future sessions know
  what to keep apart from the start.

## Sequencing reminder

Per CLAUDE.md "pre-phase chat mandatory" and "no autonomous chaining":
the audit step runs first and produces a candidate list; the user
reviews it and authorises sub-slices one at a time. Each sub-slice
gets its own pre-phase confirmation before any file moves. Refactor
commits land sequentially with push between, never batched.
