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

### Audit results (2026-05-03)

Workspace sweep against the strict rule + carve-outs identified
**15 violators totaling ~67 estimated extraction commits**. Several
files that initially looked like candidates by line count pass under
a carve-out and are left in place.

#### Violator inventory

| # | Sub-slice | File | Lines | Failed criterion | Mixed concerns | Est. commits |
|---|---|---|---:|---|---|---:|
| 1 | 20k-A2c | `corvid-runtime/src/queue/mod.rs` | 1,527 | 1, 3 | `QueueRuntime` (in-memory) + `DurableQueueRuntime` + `insert_job_audit_event` + 1,168-line cross-domain test cluster | 7 |
| 2 | 20k-A5b | `corvid-runtime/src/runtime/mod.rs` | 1,414 | 1, 3 | `Runtime` + `RuntimeBuilder` + 967-line tests across 7 sibling concerns | 7 |
| 3 | 20k-A3b | `corvid-codegen-cl/src/lowering/runtime/mod.rs` | 1,431 | 2, 3 | ~1,030-line `declare_runtime_funcs` + 70 `pub(super) const *_SYMBOL` + `RuntimeFuncs`/`TracePayload`/`LoopCtx` types + the literal-id helper | 4 |
| 4 | 20k-A6b | `corvid-codegen-cl/src/lowering/expr/mod.rs` | 1,192 | 1, 2 | 970-line `lower_expr` switch + four `pub(super)` borrow/operand helpers + `tool_wrapper_symbol` mangler | 2 |
| 5 | 20k-A13b | `corvid-runtime/src/replay/mod.rs` | 924 | 1 | `ReplaySource` + `ReplayMutation` + `ReplayApprovalDecision` records — three "what kind of replay session" types | 2 |
| 6 | 20k-A9b | `corvid-runtime/src/ffi_bridge/mod.rs` | 976 | 1, 3 | 6 replay-tool exports + 4 prompt-call exports + citation-verify + approve-sync + 500-line `call_llm_once` LLM-orchestration helper + parse helpers + system-prompt builder | 4 |
| 7 | 20k-A10c | `corvid-runtime/src/auth/mod.rs` | 764 | 1, 3 | `SessionAuthRuntime` + 18 typed records + 375-line tests across 5 sibling auth domains | 6 |
| 8 | 20k-A1b | `corvid-cli/src/cli/root.rs` | 1,369 | 2 | `Cli` + `Command` + 17 sibling `*Command` enums (Bench, Contract, Connectors, Auth, Approvals, Claim, Deploy, Upgrade, Receipt, Bundle, Trace, Abi, Approver, Capsule …) | 14 |
| 9 | 20k-A1c | `corvid-cli/src/dispatch.rs` | 1,192 | 1, 3 | Top-level `run` + nested `cmd_connectors` + `cmd_auth` + `cmd_approvals` subdomain dispatchers | 3 |
| 10 | 20k-D1 | `corvid-differential-verify/src/rewrite.rs` | 1,929 | 1, 2 | 7-rule rewrite engine + 720-line AST renderer (printer) | 1 |
| 11 | 20k-D2 | `corvid-differential-verify/src/lib.rs` | 1,020 | 1 | Tier orchestration + report rendering + divergence diffing + corpus shrinking | 3 |
| 12 | 20k-T1 | `corvid-types/src/checker/decl.rs` | 1,010 | 1, 3 | `check_agent` (440 lines) + `check_eval`/`check_test`/`check_fixture`/`check_mock` + replayability-violation collectors + extern-C ownership inference | 3 |
| 13 | 20k-R1 | `corvid-runtime/src/catalog_c_api.rs` | 1,385 | 1 | 33 `extern "C"` exports across 4 sub-domains: scalar invoke matrix + grounded-source-handle FFI + approval-decision FFI + descriptor/library-path helpers | 4 |
| 14 | 20k-IR1 | `corvid-ir/src/lib.rs` | 1,009 | 1 | 6-line crate-root facade + 991-line `#[cfg(test)] mod tests` block (32 tests covering both `lower` and `types` sibling modules — exceeds 300 AND covers siblings, so carve-out 1 fails) | 5 |
| 15 | 20k-CLI1 | `corvid-cli/src/eval_cmd.rs` | 995 | 1, 2 | `run_eval` + `run_promote_lineage` + `run_compare` + 8 `Stored*`/`CompareReport`/`*Change` records | 2 |

#### Per-violator decomposition plans

**20k-A2c — `queue/mod.rs` (1,527 → ~340)**

Move `DurableQueueRuntime` impl to `queue/durable.rs`; move
`insert_job_audit_event` + `eligible_to_run` to `queue/audit.rs`; split
the 1,168-line tests into `queue/tests/durable_basics.rs`,
`queue/tests/leases.rs`, `queue/tests/checkpoints.rs`,
`queue/tests/approvals.rs`, `queue/tests/loops.rs`,
`queue/tests/scheduler.rs`. Root keeps only `QueueRuntime` +
sub-mod declarations.

**20k-A5b — `runtime/mod.rs` (1,414 → ~450)**

Extract `RuntimeBuilder` (lines 215–446) to `runtime/builder.rs`. Split
the 967-line tests by sibling domain into `runtime/tests/store.rs`,
`runtime/tests/python.rs`, `runtime/tests/approvals.rs`,
`runtime/tests/llm.rs`, `runtime/tests/io_http.rs`,
`runtime/tests/secrets_cache.rs`.

**20k-A3b — `lowering/runtime/mod.rs` (1,431 → facade)**

Move the `*_SYMBOL` constants to `lowering/runtime/symbols.rs`. Move
`declare_runtime_funcs` to `lowering/runtime/declare.rs`. Move
`RuntimeFuncs` struct + impl to `lowering/runtime/funcs.rs`. Move
`LoopCtx`/`TracePayload` to `lowering/runtime/payload.rs`. Root
becomes a re-export facade.

**20k-A6b — `lowering/expr/mod.rs` (1,192 → ~970)**

Extract the borrow/operand helpers to `lowering/expr/operand.rs`.
Extract `tool_wrapper_symbol` to `lowering/expr/wrappers.rs`. Root
keeps `lower_expr` and its `match` body.

**20k-A13b — `replay/mod.rs` (924 → ~600)**

Extract `ReplayMutation` + `ReplayMutationState` + impl to
`replay/mutation_session.rs`. Extract approval-decision records to
`replay/approval_outcome.rs`. Root keeps `ReplaySource` + `ReplayMode`.

**20k-A9b — `ffi_bridge/mod.rs` (976 → ~250)**

Extract `call_llm_once` + `build_system_prompt` +
`trace_mock_llm_attempt` + `prompt_max_retries` +
`format_instruction_*` + `using_env_mock_llm` + `parse_int/bool/float`
+ `strip_response` to `ffi_bridge/llm_dispatch.rs`. Extract
`corvid_prompt_call_*` exports to `ffi_bridge/prompt_exports.rs`.
Extract `corvid_replay_tool_call_*` exports to
`ffi_bridge/replay_exports.rs`. Extract `corvid_approve_sync` +
`corvid_citation_verify_or_panic` to
`ffi_bridge/approval_exports.rs`.

**20k-A10c — `auth/mod.rs` (764 → ~150)** *(pattern reference; see
section below for full per-commit plan)*

Already named in 20j's closing audit. Records → `auth/records.rs`;
tests split per domain into `auth/tests/sessions.rs`,
`auth/tests/api_keys.rs`, `auth/tests/jwt.rs`,
`auth/tests/oauth.rs`, `auth/tests/permissions.rs`. Root keeps the
actor surface + DDL + module wiring.

**20k-A1b — `cli/root.rs` (1,369 → ~150)**

Per-group submodules: `cli/bench.rs`, `cli/contract.rs`,
`cli/connectors.rs` (with nested `oauth`), `cli/auth.rs` (with
`keys`), `cli/approvals.rs`, `cli/claim.rs`, `cli/deploy.rs`,
`cli/upgrade.rs`, `cli/receipt.rs`, `cli/bundle.rs`, `cli/trace.rs`,
`cli/abi.rs`, `cli/approver.rs`, `cli/capsule.rs`. Root keeps only
`Cli` + `Command`.

**20k-A1c — `dispatch.rs` (1,192 → ~600)**

Extract each `cmd_*` to its own dispatch file:
`dispatch/connectors.rs`, `dispatch/auth.rs`,
`dispatch/approvals.rs`. Root keeps only `pub(crate) fn run` plus
re-exports.

**20k-D1 — `differential-verify/rewrite.rs` (1,929 → ~1,200)**

Move all `render_*` AST printer functions (lines 1209–1929) to
`corvid-differential-verify/src/render.rs`. The seven rule engines
stay together.

**20k-D2 — `differential-verify/lib.rs` (1,020 → ~400)**

Extract rendering helpers + `*_rank` to
`corvid-differential-verify/src/render.rs` (combining with D1's
extracted printer). Extract divergence diffing (`diff_reports`,
`maybe_push_divergence`, `*_overapproximation`, `*_too_loose`,
profile helpers, `value_map`) to
`corvid-differential-verify/src/diff.rs`. Move `shrink_program` to
`corvid-differential-verify/src/shrink.rs`. Root keeps tier
orchestration + `Frontend` + the four `*_report` runners.

**20k-T1 — `types/checker/decl.rs` (1,010 → ~470)**

Extract `check_eval`/`check_test`/`check_fixture`/`check_mock` plus
their assertion helpers to `checker/decl_eval.rs`. Move extern-C
ownership inference (`check_extern_c_signature`,
`extern_c_param_type_supported`, `infer_extern_*_ownership`,
`ownership_*`, `InferredOwnership`) to `checker/decl_extern_c.rs`.
Move `ReplayabilityViolation` collectors to
`checker/decl_replayability.rs`. Root keeps `check_agent` and its
agent-specific helpers.

**20k-R1 — `runtime/catalog_c_api.rs` (1,385 → ~400)**

Extract scalar-invocation matrix (`impl_invoke1!`,
`impl_invoke2_matrix!`, `parse_*_arg`, `float_json`) to
`catalog_c_api/invoke_matrix.rs`. Extract approval bridging
(`ApproverRegistration`, `owned_approval_to_c`,
`owned_preflight_to_c`, `record_host_event`,
`LAST_APPROVAL_DETAIL`, `PREAPPROVED_REQUESTS`) to
`catalog_c_api/approval_bridge.rs`. Extract grounded-handle pointers
(`grounded_source_pointers`, transient string TLS) to
`catalog_c_api/grounded_bridge.rs`. Root keeps
`current_library_path` + descriptor exports.

**20k-IR1 — `corvid-ir/src/lib.rs` (1,009 → ~10)**

Move the entire 991-line `#[cfg(test)] mod tests` block to
`corvid-ir/src/tests/mod.rs`, splitting per concern:
`tests/lower_basic.rs`, `tests/lower_effects.rs`,
`tests/lower_replay.rs`, `tests/lower_imports.rs`, `tests/types.rs`.
Root collapses to its 6-line shim.

**20k-CLI1 — `cli/eval_cmd.rs` (995 → ~470)**

Extract `run_compare` + all `Stored*`/`CompareReport`/`PromptChange`/
`RegressionCluster`/`AssertionChange` types and helpers
(`build_compare_report`, `cluster_regressions`, `index_summaries`,
`prompt_render_*`, `set_diff`, `percent`) to `eval_cmd/compare.rs`.
Extract `run_promote_lineage` + `read_lineage_events` +
`latest_summary_path_for_source` + `sanitize_*` helpers to
`eval_cmd/promote.rs`. Root keeps `run_eval` + `run_golden_trace_evals`
+ `run_source_evals` + spend/budget helpers.

#### Files that look like violators but pass under a carve-out

These were enumerated and judged rubric-clean; they don't need work:

- `corvid-vm/src/interp.rs` (1,056) — `Interpreter<'ir>` + inherent
  impl. Carve-out 2.
- `corvid-resolve/src/resolver.rs` (1,042) — `Resolver` + inherent
  impl. Carve-out 2.
- `corvid-ir/src/lower.rs` (1,407) — `Lowerer<'a>` + inherent impl.
  Carve-out 2.
- `corvid-driver/src/modules.rs` (1,455) — single concept (cross-file
  `.cor` import loader); 590-line tests cover only the loader's own
  behavior. Carve-out 1.
- `corvid-guarantees/src/registry.rs` (1,148) — one
  `pub static GUARANTEE_REGISTRY` table plus four lookup helpers
  reading the same table. One responsibility ("the registry data and
  its readers").
- `corvid-abi/src/emit.rs` (946) — single concept (emit `CorvidAbi`
  from IR); all `emit_*` helpers feed `emit_abi`.
- `corvid-runtime/src/approval_queue.rs` (866) —
  `ApprovalQueueRuntime` + inherent impl + 397-line tests covering
  only the queue's own behavior. Carve-out 1 (just over 300 but
  single-concept).
- `corvid-codegen-cl/src/dup_drop.rs` (1,002) — `insert_dup_drop`
  algorithm + 275-line tests testing only the algorithm. Carve-out 1.
- `corvid-runtime/src/models.rs` (868) — `RegisteredModel` +
  `ModelCatalog` form one cohesive "model catalog" facade; 282-line
  tests under 300. Carve-outs 2 + 1.
- `corvid-ast/src/decl.rs` (880) — dense AST declaration types; all
  share the single concern "AST decl tree."
- `corvid-repl/src/lib.rs` (2,345) — `Repl` + inherent impl forms
  the bulk; supporting `display_*`/`format_*`/`history_*` are thin
  formatter helpers consumed only by `Repl`. Carve-out 2.
- `corvid-driver/src/build/server_render.rs` (884) — one concept
  (render embedded server source for a Corvid HTTP target);
  duplicated `fn` definitions are inside `format!` templates, not
  real top-level fns.
- `corvid-bind/src/python_backend.rs` (893) — all `render_*` helpers
  feed one Python-binding generator.

### 20k-A10c — auth records and tests split (pattern reference)

Already sketched above; the per-commit plan stays:

1. `extract records from auth` → `auth/records.rs` holds the 16
   typed records.
2. `relocate session tests to sessions` — the four
   `session_runtime_*` / `session_rotation_*` tests move into
   `sessions.rs`'s `#[cfg(test)] mod tests`.
3. `relocate api_key tests to api_keys` — the two
   `api_key_runtime_*` tests move into `api_keys.rs`.
4. `relocate oauth tests to oauth` — the three `oauth_*` tests move
   into `oauth.rs`.
5. `relocate jwt + permission tests` — `jwt_contract_validation_*`
   and `permission_propagation_*` tests find their best-fitting
   sibling.
6. `collapse auth mod to actor surface` — what remains is the actor
   surface + DDL + module wiring + `validate_non_empty`. Target:
   ~150 lines.

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
