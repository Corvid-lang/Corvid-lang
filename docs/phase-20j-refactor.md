# Phase 20j — File-Responsibility Audit & Refactor

Rubric sweep of every `.rs` source file in the workspace, conducted
2026-04-30 against the CLAUDE.md responsibility rubric. Phase 20i
(2026-04-19) closed at the time with every monster decomposed; this
phase corrects post-20i regrowth in `corvid-runtime`, the
`corvid-codegen-cl` lowering subtree, `corvid-driver`, and
`corvid-cli`, plus four files I shipped this session at sizes that
already failed the rubric.

## Why this phase exists

CLAUDE.md says:

> Every source file holds 1–2 responsibilities. Line count is a
> heuristic for where to look — it is not the rule.
>
> A file fails this discipline when any of these hold:
> 1. It mixes unrelated top-level concepts (parsing + lexing;
>    checking + IR lowering; dispatch + recording).
> 2. It has 5+ public items representing unrelated domains.
> 3. It has 3+ internal sections that share no state.

A second-pass audit on 2026-04-30 surfaced 36 files in the workspace
that fail at least one of these criteria — five named by the user
directly, plus 31 found by a workspace-wide `general-purpose` agent
sweep. Phase 20j enumerates each, names the rubric criterion failed,
and proposes a target decomposition.

Per Phase 20i's precedent, every split is bound by:

- **One commit per file extraction.** No batching.
- **Validation gate between every commit:** `cargo check
  --workspace` + targeted `cargo test -p <crate> --lib` + `cargo
  run -q -p corvid-cli -- verify --corpus tests/corpus` (the
  pre-existing `whoami` linker error in `corvid-driver --lib` is
  ignored — it predates this phase and is environmental).
- **Push before starting the next extraction.**
- **Wait for acknowledgement at slice boundaries** in parallel
  work — no autonomous chaining.
- **Zero semantic changes during a refactor commit.** Move code,
  add `pub use` re-exports to preserve the public API, nothing
  else. Bugs spotted mid-refactor get a separate branch.
- **Commit message format:** `refactor(<crate>): extract
  <responsibility> from <file>` — body names which rubric
  criterion failed and how the split resolves it.

## Sequencing strategy

The 36 files are ordered into four tiers. Each tier is one slice of
Phase 20j; each slice ships its own ROADMAP-tracked sub-checklist.

**Tier S — Session-introduced retro-splits (4 files, ~12 commits).**
Files I shipped during 2026-04-29's audit-correction track that
were oversized at land time. CLAUDE.md says when modifying a file
for a feature, split first as a separate commit *preceding* the
feature commit. I did not. These are atonement.

**Tier A — Large monoliths ≥1,500 lines (14 files, ~80 commits).**
Includes the user-named-5 plus 9 audit-discovered monsters. These
front entire subsystems and need 4–12 extraction commits each.

**Tier B — Medium grab-bags 700–1,500 lines (14 files, ~52 commits).**
3–5 unrelated leaves per file. Mostly CLI command files and runtime
managers that fold their typed records, persistence, and helpers
into one module.

**Tier C — Smaller-but-mixed (4 files, ~12 commits).** Below 700
lines but failing criterion 2 (5+ unrelated public items). Often
fixable with a single helper-extraction commit each.

**Total estimate: ~156 commits across 4 slices.** Phase 20i closed
~60 commits over a similar span. Cadence target: one tier per
~1–2 weeks, slices land sequentially with pre-phase chat at each
boundary.

## Inventory

Severity column: `S` = session-introduced; `M` = monolith ≥1,500
lines; `G` = grab-bag 700–1,500; `X` = small-but-mixed.

| # | File | Lines | Tier | Rubric | Mixed concerns |
|---|------|-------|------|--------|----------------|
| 1 | `crates/corvid-cli/src/auth_cmd.rs` | 920 | S | 1 | `auth migrate` + `auth keys issue/revoke/rotate` + `approvals queue/inspect/approve/deny/expire/comment/delegate/batch/export` — two unrelated CLI surfaces (auth + approvals) on one file |
| 2 | `crates/corvid-cli/src/connectors_cmd.rs` | 941 | S | 1 | Six independent subcommands (`list`, `check`, `run`, `oauth init`, `oauth rotate`, `verify-webhook`) with their own Args/Output structs |
| 3 | `crates/corvid-cli/src/observe_helpers_cmd.rs` | 1,083 | S | 1+2 | 4 unrelated subcommands (`observe explain`, `observe cost-optimise`, `eval drift --explain`, `eval generate-from-feedback`); 16+ public items spanning incident root cause / cost suggestion / drift attribution / fixture synthesis |
| 4 | `crates/corvid-runtime/src/jwt_verify.rs` | 694 | S | 2 | `JsonWebKey`/`JsonWebKeySet` types + `JwksFetcher` trait + `ReqwestJwksFetcher` HTTP impl + `JwtVerifyError` + `VerifiedJwtClaims` + `JwtVerifier` |
| 5 | `crates/corvid-cli/src/main.rs` | 5,299 | M | 1+3 | CLI parsing + dispatch + migrations + jobs + auth + approvals + doctor + package + build/run + helpers |
| 6 | `crates/corvid-runtime/src/queue.rs` | 3,748 | M | 1+3 | queue models + SQLite persistence + scheduling + leases + retries + approvals + checkpoints + loop limits + stall detection + tests |
| 7 | `crates/corvid-codegen-cl/src/lowering/runtime.rs` | 3,220 | M | 1+3 | runtime symbol decl + stackmap + struct/result/option destructors + trace + typeinfo + retain/release + type predicates |
| 8 | `crates/corvid-driver/src/lib.rs` | 2,670 | M | 1+2 | 6 separate `compile_*` entry points + render glue + 8 sub-module declarations |
| 9 | `crates/corvid-runtime/src/runtime.rs` | 2,590 | M | 1+2+3 | One `impl Runtime` (~1,272 lines, ~70 methods) covering tools, tracer, recorder, replay, model catalog, IO, jobs, store, FFI |
| 10 | `crates/corvid-codegen-cl/src/lowering/expr.rs` | 2,428 | M | 1 | constructors + binop strict + binop wrapping + overflow trap + try-propagate + try-retry |
| 11 | `crates/corvid-driver/src/build.rs` | 1,784 | M | 1 | build orchestration + signed claim coverage + wasm/server/native output + server source rendering + catalog descriptors |
| 12 | `crates/corvid-guarantees/src/lib.rs` | 1,639 | M | 1+3 | public types + registry data + validation + signed-claim ids + tests |
| 13 | `crates/corvid-runtime/src/ffi_bridge.rs` | 1,580 | M | 1+3 | C-ABI state + tokio handle + retain/release + string factories + tool iter |
| 14 | `crates/corvid-runtime/src/auth.rs` | 1,557 | M | 1+3 | API keys + sessions + OAuth + approvals + tenancy + audit + tests |
| 15 | `crates/corvid-syntax/src/parser/decl.rs` | 1,549 | M | 1+2 | 14+ unrelated decl families on one `Parser` impl |
| 16 | `crates/corvid-bind/src/rust_backend.rs` | 1,259 | M | 1+3 | renders Cargo.toml + README + lib.rs + common.rs + types.rs + catalog.rs + agent modules |
| 17 | `crates/corvid-runtime/src/replay/mod.rs` | 1,208 | M | 1+3 | One `impl ReplaySource` (~736 lines) mixing load + cursor + dispatch + JSON factory + validation |
| 18 | `crates/corvid-vm/src/value.rs` | 1,193 | M | 1+2+3 | value model + heap mgmt + stream IO + display + weak refs |
| 19 | `crates/corvid-vm/src/interp.rs` | 1,259 | G | 1+2 | dispatch + run_validate + grounding alongside ~6 submodule decls |
| 20 | `crates/corvid-codegen-cl/src/dataflow.rs` | 1,255 | G | 1+3 | CFG + liveness + ownership-plan + branch-drop |
| 21 | `crates/corvid-vm/src/interp/prompt.rs` | 1,249 | G | 1 | voting + adversarial + route dispatch + cost charging |
| 22 | `crates/corvid-runtime/src/approval_queue.rs` | 1,156 | G | 1+3 | record models + SQLite + audit + runtime |
| 23 | `crates/corvid-runtime/src/rag.rs` | 1,155 | G | 1+3 | types + 2 embedders + 3 loaders + chunking + SQLite index |
| 24 | `crates/corvid-cli/src/test_from_traces.rs` | 1,100 | G | 1 | load + filter + render + dispatch + promote |
| 25 | `crates/corvid-driver/src/eval_runner.rs` | 1,086 | G | 1+2 | 6 record types + options + render |
| 26 | `crates/corvid-driver/src/package_registry.rs` | 1,086 | G | 1 | add + remove + update + publish + verify |
| 27 | `crates/corvid-cli/src/trace_diff/stacked.rs` | 1,078 | G | 1 | normal-form + history + anomaly + bisection |
| 28 | `crates/corvid-runtime/src/catalog.rs` | 1,035 | G | 1+2 | 12 types + descriptor introspection + invocation + filter |
| 29 | `crates/corvid-types/src/errors.rs` | 1,019 | G | 2 | error kind + warning kind + 540-line Display impls |
| 30 | `crates/corvid-runtime/src/store.rs` | 988 | G | 1+2 | trait + manager + 2 backends + 8 policy parsers |
| 31 | `crates/corvid-shadow-daemon/src/replay_pool.rs` | 830 | G | 1+2 | pool + 2 executors + IR-parsing + 6 typed records |
| 32 | `crates/corvid-runtime/src/approver_bridge.rs` | 760 | G | 1 | source compile + ABI emission + state mgmt + simulate |
| 33 | `crates/corvid-types/src/effects/cost.rs` | 744 | G | 1 | analysis + render + query |
| 34 | `crates/corvid-cli/src/replay.rs` | 709 | X | 1 | 3 replay modes (plain + differential + mutation) |
| 35 | `crates/corvid-runtime/src/approvals.rs` | 702 | X | 1+2 | data + token + 2 approvers + benchmark hook |
| 36 | `crates/corvid-cli/src/observe_cmd.rs` | 691 | X | 1 | list + show + drift × (build + render) |
| 37 | `crates/corvid-cli/src/routing_report.rs` | 676 | X | 1 | record + git ingest + render |

## Slice 20j-S — Session-introduced retro-splits (4 files, ~12 commits)

Goal: close the rubric breaches I introduced this session before any
new feature work.

### 20j-S1 — `corvid-cli/src/auth_cmd.rs` (920 → ~80 lines, 4 commits)

**Rubric**: criterion 1. Two unrelated CLI surfaces (auth + approvals)
share a file because they were landed together in slice 39L.

**Decomposition:**

```
auth_cmd/
├── mod.rs                # re-exports for main.rs dispatch
├── migrate.rs            # `corvid auth migrate`
├── keys.rs               # `corvid auth keys issue/revoke/rotate`
└── support.rs            # tenant/actor lookup helpers shared by both

approvals_cmd/
├── mod.rs
├── queue.rs              # `corvid approvals queue/inspect/export`
├── transition.rs         # `corvid approvals approve/deny/expire`
└── interaction.rs        # `corvid approvals comment/delegate/batch`
```

**Per-extraction commits (in order):**
1. `refactor(cli): extract auth_cmd::keys from auth_cmd`
2. `refactor(cli): extract approvals_cmd::queue from auth_cmd`
3. `refactor(cli): extract approvals_cmd::transition from auth_cmd`
4. `refactor(cli): extract approvals_cmd::interaction from auth_cmd`

`auth_cmd::migrate` and `auth_cmd::support` stay in the original file
since they're tightly coupled to the migrator.

### 20j-S2 — `corvid-cli/src/connectors_cmd.rs` (941 → ~80 lines, 5 commits)

**Rubric**: criterion 1. Six independent subcommands.

**Decomposition:**

```
connectors_cmd/
├── mod.rs                # re-exports + dispatch glue
├── list.rs               # `corvid connectors list`
├── check.rs              # `corvid connectors check [--live]`
├── run.rs                # `corvid connectors run`
├── oauth.rs              # `corvid connectors oauth init|rotate`
├── verify_webhook.rs     # `corvid connectors verify-webhook`
└── support.rs            # `shipped_manifests`, `random_b64url_bytes`,
                          # `pkce_code_challenge`, `url_encode`,
                          # `summarise_manifest`
```

**Per-extraction commits:**
1. `refactor(cli): extract connectors_cmd::support from connectors_cmd`
2. `refactor(cli): extract connectors_cmd::list from connectors_cmd`
3. `refactor(cli): extract connectors_cmd::check from connectors_cmd`
4. `refactor(cli): extract connectors_cmd::run from connectors_cmd`
5. `refactor(cli): extract connectors_cmd::oauth + verify_webhook from connectors_cmd`

(Bundling `oauth` + `verify_webhook` in one commit because they share
the `WebhookVerifyOutput` rendering and the OAuth output enum.)

### 20j-S3 — `corvid-cli/src/observe_helpers_cmd.rs` (1,083 → ~60 lines, 4 commits)

**Rubric**: criterion 1+2. 4 unrelated subcommands; 16+ public items.

**Decomposition:**

```
observe_helpers_cmd/
├── mod.rs                # re-exports + dispatch glue
├── observe_explain.rs    # ObserveExplainArgs + run_observe_explain + render
├── cost_optimise.rs      # CostOptimiseArgs + run_observe_cost_optimise + render
├── eval_drift.rs         # EvalDriftArgs + run_eval_drift_explain + render
└── eval_from_feedback.rs # EvalFromFeedbackArgs + run_eval_generate_from_feedback + render
```

**Per-extraction commits:** one per leaf subcommand (4 commits). The
shared `Grounded<T>`-shape JSON helpers stay in `mod.rs` since all
four subcommands consume them.

### 20j-S4 — `corvid-runtime/src/jwt_verify.rs` (694 → ~30 lines, 2 commits)

**Rubric**: criterion 2. Mix of types + trait + HTTP fetcher impl +
verifier in one file.

**Decomposition:**

```
jwt_verify/
├── mod.rs                # JwtVerifyError, VerifiedJwtClaims, re-exports
├── jwks.rs               # JsonWebKey, JsonWebKeySet, JwksFetcher trait,
                          # ReqwestJwksFetcher
└── verifier.rs           # JwtVerifier (parse_alg, decoding_key_for, verify)
```

**Per-extraction commits:**
1. `refactor(runtime): extract jwt_verify::jwks from jwt_verify`
2. `refactor(runtime): extract jwt_verify::verifier from jwt_verify`

## Slice 20j-A — Large monoliths (14 files, ~80 commits)

Tier A files front entire subsystems. Each gets its own sub-slice
(20j-A1 through 20j-A14) since the splits are non-trivial and need
dedicated pre-phase chat.

### 20j-A1 — `corvid-cli/src/main.rs` (5,299 → ~30 lines, 12 commits)

**Rubric**: criterion 1+3. The single biggest violator in the
workspace.

**Decomposition:**

```
src/
├── main.rs               # ~30 lines: parse args, dispatch::run(cmd), exit
├── cli/                  # clap argument tree
│   ├── mod.rs            # `Cli`, `Command` (root enum)
│   ├── jobs.rs           # `JobsCommand`
│   ├── approvals.rs      # already external (auth_cmd)
│   ├── package.rs        # `PackageCommand`
│   ├── observe.rs        # `ObserveCommand`
│   ├── eval.rs           # `EvalCommand`
│   ├── connectors.rs     # already external (connectors_cmd)
│   ├── auth.rs           # already external (auth_cmd)
│   └── contract.rs       # `ContractCommand`
├── dispatch.rs           # the giant `match cmd { ... }` (~600 lines)
├── migrate_cmd.rs        # `corvid migrate run/list/up/down`
├── doctor_cmd.rs         # `corvid doctor`
├── build_cmd.rs          # `corvid build`
├── run_cmd.rs            # `corvid run`
├── verify_cmd.rs         # `corvid verify`
├── package_cmd.rs        # `corvid package install/lock/publish/...`
├── format.rs             # JSON/text output helpers shared across commands
└── (existing leaf modules untouched)
```

**Per-extraction commits (in order, smallest blast-radius first):**
1. `refactor(cli): extract format helpers from main`
2. `refactor(cli): extract cli::jobs argument tree from main`
3. `refactor(cli): extract cli::package argument tree from main`
4. `refactor(cli): extract cli::observe + cli::eval argument trees from main`
5. `refactor(cli): extract migrate_cmd from main`
6. `refactor(cli): extract doctor_cmd from main`
7. `refactor(cli): extract verify_cmd from main`
8. `refactor(cli): extract build_cmd from main`
9. `refactor(cli): extract run_cmd from main`
10. `refactor(cli): extract package_cmd from main`
11. `refactor(cli): extract dispatch from main`
12. `refactor(cli): collapse main to entry point`

**Validation gate per commit**: `cargo check --workspace` + `cargo
test -p corvid-cli` + `cargo run -q -p corvid-cli -- --help`
(every subcommand still resolves through the dispatcher).

### 20j-A2 — `corvid-runtime/src/queue.rs` (3,748 → ~80 lines, 9 commits)

**Rubric**: criterion 1+3. Mixes queue model, persistence,
scheduling, leases, retries, approvals, checkpoints, loop limits,
stall detection.

**Decomposition:**

```
queue/
├── mod.rs                # re-exports + DurableQueueRuntime entry
├── model.rs              # QueueJob, JobLease, JobStatus, IdempotencyKey types
├── sqlite.rs             # SQLite schema + persistence methods
├── lease.rs              # lease_next_at, complete_leased, fail_leased, heartbeat
├── retry.rs              # retry budget, backoff, dead-letter
├── schedule.rs           # cron iteration, missed_fire_times, FireOncePolicy
├── checkpoint.rs         # checkpoint write/read, durable resume
├── approval.rs           # approval-wait state, resume on transition
└── loops.rs              # loop bounds, stall detection
```

**Per-extraction commits:** model → sqlite → schedule → lease →
retry → checkpoint → approval → loops → mod.rs collapse (9 commits).

### 20j-A3 — `corvid-codegen-cl/src/lowering/runtime.rs` (3,220 → ~80 lines, 6 commits)

**Rubric**: criterion 1+3. Bundles runtime-symbol declaration
with stackmap emission, struct/result/option destructors, trace,
typeinfo, retain/release, and type predicates.

**Decomposition:**

```
lowering/runtime/
├── mod.rs
├── decl.rs               # runtime symbol decl (already extracted in 20i-5; verify)
├── stackmap.rs           # stack-map table emission
├── destructors.rs        # struct/result/option destructor synth
├── trace.rs              # struct/result/option trace impls
├── typeinfo.rs           # struct/result/option/list typeinfo
└── type_query.rs         # native-type predicates, type-name mangling, retain/release
```

**Per-extraction commits:** stackmap → destructors → trace →
typeinfo → type_query → collapse (6 commits).

### 20j-A4 — `corvid-driver/src/lib.rs` (2,670 → ~200 lines, 8 commits)

**Rubric**: criterion 1+2. 6 `compile_*` entry points + render glue +
8 sub-module declarations all in one root file.

**Decomposition:**

```
driver/src/
├── lib.rs                # ~200 lines: crate surface + mod declarations + a single re-exported `compile`/`compile_with_config` facade
├── pipeline/             # all `compile_*` entry points
│   ├── mod.rs
│   ├── compile.rs        # compile, compile_with_config, compile_with_config_at_path
│   ├── ir.rs             # compile_to_ir, compile_to_ir_with_config*
│   └── abi.rs            # compile_to_abi_with_config
├── config_loader.rs      # load_corvid_config_for
└── (existing add_dimension/, adversarial/, approver/, effect_diff/,
     meta_verify/, modules/, proof_replay/, package_*/, native_*/,
     render/, spec_*/ stay as-is — they're already extracted)
```

**Per-extraction commits:** config_loader → pipeline::compile →
pipeline::ir → pipeline::abi → trim lib.rs to facade (5 commits;
the existing sub-modules survived 20i and don't move).

### 20j-A5 — `corvid-runtime/src/runtime.rs` (2,590 → ~400 lines, 6 commits)

**Rubric**: criterion 1+2+3. Post-20i regrowth from 445 lines to
2,590 (5.8×) — the worst single regression of any 20i-clean file.
70-method `impl Runtime` covers tools, tracer, recorder, replay,
model catalog, IO, jobs, store, FFI.

**Decomposition:**

```
runtime/
├── mod.rs                # ~400 lines: Runtime + RuntimeBuilder dispatch glue + the methods that genuinely belong on the type (init, shutdown, basic accessors)
├── llm_dispatch.rs       # LLM call methods + provider selection + cache keys
├── replay_reports.rs     # observation summary + replay reporting
├── io.rs                 # read/write/list, env secrets
├── store_dispatch.rs     # store get/put/delete/policy methods
├── jobs.rs               # enqueue/cancel/job-state methods
└── model_catalog.rs      # model catalog + provider health
```

**Per-extraction commits:** llm_dispatch → replay_reports → io →
store_dispatch → jobs → model_catalog (6 commits, in order of
smallest method count first).

### 20j-A6 — `corvid-codegen-cl/src/lowering/expr.rs` (2,428 → ~400 lines, 5 commits)

**Rubric**: criterion 1.

**Decomposition:**

```
lowering/expr/
├── mod.rs                # ~400 lines: expression dispatch + container lowering
├── binop.rs              # binop strict + binop wrapping + arithmetic promotion + unop
├── constructors.rs       # struct/result/option constructors + string literal
├── overflow.rs           # overflow trap + zero trap
├── try_propagate.rs      # try-propagate (option / result)
└── try_retry.rs          # try-retry (option / result)
```

**Per-extraction commits:** constructors → binop → overflow →
try_propagate → try_retry (5 commits).

### 20j-A7 — `corvid-driver/src/build.rs` (1,784 → ~400 lines, 4 commits)

**Rubric**: criterion 1.

**Decomposition:**

```
build/
├── mod.rs                # ~400 lines: BuildTarget enum + per-target dispatch
├── claim_coverage.rs     # validate_signed_claim_coverage + DeclaredContractClaims + collect_*_contracts
├── server_render.rs      # generated server source rendering
├── catalog_descriptor.rs # CatalogDescriptorOutput + descriptor emission
└── tests.rs              # tests already exist; mostly relocated
```

**Per-extraction commits:** claim_coverage → server_render →
catalog_descriptor → tests (4 commits).

### 20j-A8 — `corvid-guarantees/src/lib.rs` (1,639 → ~50 lines, 4 commits)

**Rubric**: criterion 1+3. Plan was already drafted in the
preceding chat.

**Decomposition:**

```
guarantees/src/
├── lib.rs                # ~50 lines: pub use re-exports
├── types.rs              # Phase, GuaranteeKind, GuaranteeClass, Guarantee, slug() impls
├── registry.rs           # GUARANTEE_REGISTRY + lookup/iter/by_class/by_kind
├── signed_claim.rs       # SIGNED_CDYLIB_CLAIM_GUARANTEE_IDS + signed_cdylib_claim_guarantees
├── validate.rs           # RegistryError + validate_slice + validate_id_shape + split_test_ref + read_file_under_workspace
└── render.rs             # already extracted (markdown rendering)
```

**Per-extraction commits:**
1. `refactor(guarantees): extract types.rs from lib`
2. `refactor(guarantees): extract validate.rs from lib`
3. `refactor(guarantees): extract registry.rs from lib`
4. `refactor(guarantees): extract signed_claim.rs from lib`

### 20j-A9 — `corvid-runtime/src/ffi_bridge.rs` (1,580 → ~200 lines, 4 commits)

**Rubric**: criterion 1+3. Post-20i regrowth from 1,079 to 1,580
(1.5×). 20i marked it clean ("the concern is singular") because
the FFI surface was one monolithic concern; new growth crossed the
rubric.

**Decomposition:**

```
ffi/
├── mod.rs                # ~200 lines: extern "C" entry points (crate-public surface)
├── state.rs              # CorvidBridgeState, runtime handle mgmt
├── tokio_handle.rs       # tokio runtime init + handle borrow
├── strings.rs            # CorvidString factories + retain/release
└── tool_iter.rs          # tool iterator + per-tool descriptor exposure
```

**Per-extraction commits:** state → tokio_handle → strings →
tool_iter (4 commits).

### 20j-A10 — `corvid-runtime/src/auth.rs` (1,557 → ~150 lines, 5 commits)

**Rubric**: criterion 1+3. API keys + sessions + OAuth + approvals
+ tenancy + audit on one file.

**Decomposition:**

```
auth/
├── mod.rs                # ~150 lines: Auth facade + cross-cutting types (Tenant, Actor)
├── api_keys.rs           # Argon2 hashing + API key issuance/revocation
├── sessions.rs           # session token mint/validate + refresh
├── oauth.rs              # OAuth state machine + PKCE + provider config
├── approvals.rs          # approval contract validation (NOT to be confused with corvid-runtime/src/approvals.rs which is the data model)
└── audit.rs              # auth audit-event emission
```

**Per-extraction commits:** api_keys → sessions → oauth → approvals
→ audit (5 commits).

### 20j-A11 — `corvid-syntax/src/parser/decl.rs` (1,549 → ~150 lines, 8 commits)

**Rubric**: criterion 1+2. 14+ decl families on one `Parser` impl.

**Decomposition:**

```
parser/decl/
├── mod.rs                # ~150 lines: top-level parse_decl dispatch
├── import.rs             # import decls
├── type_field.rs         # type + field decls
├── tool.rs               # tool decls
├── effect_dimension.rs   # effect + dimension decls
├── model.rs              # model decls
├── agent_eval.rs         # agent + eval/test decls
├── server_route.rs       # schedule + server + http_route decls
└── store_extend.rs       # store + extend + fixture + mock decls
```

**Per-extraction commits:** import → type_field → tool →
effect_dimension → model → agent_eval → server_route →
store_extend (8 commits).

### 20j-A12 — `corvid-bind/src/rust_backend.rs` (1,259 → ~150 lines, 5 commits)

**Rubric**: criterion 1+3. Renders 7 distinct generated artifacts
in one file.

**Decomposition:**

```
rust_backend/
├── mod.rs                # ~150 lines: emit_rust_backend entry + per-artifact dispatch
├── cargo.rs              # Cargo.toml template
├── lib_template.rs       # generated lib.rs template
├── common_template.rs    # generated common.rs template (TrustTier, CallStatus, ApprovalRequest, etc.)
├── types_template.rs     # generated types.rs template
└── agent_emit.rs         # per-agent module emission
```

(README is small enough to stay in mod.rs.)

**Per-extraction commits:** cargo → lib_template → common_template
→ types_template → agent_emit (5 commits).

### 20j-A13 — `corvid-runtime/src/replay/mod.rs` (1,208 → ~250 lines, 3 commits)

**Rubric**: criterion 1+3. ~736-line `impl ReplaySource` mixes
loading + cursor + dispatch + JSON factory + validation.

**Decomposition:**

```
replay/
├── mod.rs                # ~250 lines: ReplaySource public API + load/cursor mgmt
├── result_factory.rs     # JSON-result production for tools/LLM/approval
├── mutation_validate.rs  # mutation classification + validation
└── event_classify.rs     # event-kind classification + display helpers
```

**Per-extraction commits:** result_factory → mutation_validate →
event_classify (3 commits).

### 20j-A14 — `corvid-vm/src/value.rs` (1,193 → ~400 lines, 4 commits)

**Rubric**: criterion 1+2+3. Post-20i regrowth from 971 to 1,193.

**Decomposition:**

```
value/
├── mod.rs                # ~400 lines: Value enum + GroundedValue + PartialValue + ResumeTokenValue (the runtime value model proper)
├── heap.rs               # Object, StructInner, ListInner, BoxedValue (heap allocation + Drop)
├── stream.rs             # StreamValue, StreamSender, StreamChunk
├── weak.rs               # WeakValue family
└── display.rs            # escape_display + value_confidence + Debug/Display impls
```

**Per-extraction commits:** display → weak → stream → heap (4
commits).

## Slice 20j-B — Medium grab-bags (14 files, ~52 commits)

Each file gets a concise plan; one commit per major extraction.
Detailed pre-phase chat per sub-slice as we land them.

### 20j-B1 — `corvid-vm/src/interp.rs` (1,259 → ~700 lines, 2 commits)

20i closed it at 779 lines; new growth pushed it to 1,259. Extract
`run_validate.rs` and `grounding.rs`. The submodules
(`effect_compose`, `expr`, `prompt`, `replay`, `stmt`,
`stream_ops`, `test_runner`, `test_trace`) are already split.

### 20j-B2 — `corvid-codegen-cl/src/dataflow.rs` (1,255 → ~300 lines, 3 commits)

Extract `cfg.rs`, `liveness.rs`, `ownership_plan.rs`. 20i's
banner-section pattern made this an obvious split target —
dataflow.rs has 8+ `// -----` banners.

### 20j-B3 — `corvid-vm/src/interp/prompt.rs` (1,249 → ~400 lines, 4 commits)

20i closed it at 912 lines clean; recent growth added
adversarial dispatch + cost charging + route patterns. Extract
`voting.rs`, `adversarial.rs`, `route_dispatch.rs`, `cost.rs`.

### 20j-B4 — `corvid-runtime/src/approval_queue.rs` (1,156 → ~250 lines, 3 commits)

Extract `records.rs` (typed records), `audit.rs`
(ApprovalAuditCoverage + audit emission), `sqlite.rs` (persistence).

### 20j-B5 — `corvid-runtime/src/rag.rs` (1,155 → ~150 lines, 4 commits)

Extract `types.rs`, `embedders.rs` (OpenAI + Ollama), `loaders.rs`
(markdown/html/pdf), `chunk.rs`, `index.rs` (SQLite).

### 20j-B6 — `corvid-cli/src/test_from_traces.rs` (1,100 → ~200 lines, 3 commits)

Extract `load.rs`, `render.rs`, `promote.rs`.

### 20j-B7 — `corvid-driver/src/eval_runner.rs` (1,086 → ~400 lines, 2 commits)

Extract `report.rs` (record types) and `render.rs` (rendering).

### 20j-B8 — `corvid-driver/src/package_registry.rs` (1,086 → ~150 lines, 5 commits)

Extract one file per registry op: `add.rs`, `remove.rs`,
`update.rs`, `publish.rs`, `verify.rs`.

### 20j-B9 — `corvid-cli/src/trace_diff/stacked.rs` (1,078 → ~250 lines, 3 commits)

Extract `normal_form.rs`, `history.rs`, `anomaly.rs`.

### 20j-B10 — `corvid-runtime/src/catalog.rs` (1,035 → ~150 lines, 4 commits)

Extract `types.rs` (typed records), `descriptor.rs` (descriptor
introspection), `invoke.rs` (agent invocation), `filter.rs`
(`find_agents_where`).

### 20j-B11 — `corvid-types/src/errors.rs` (1,019 → ~150 lines, 3 commits)

Extract `error_kind.rs` (TypeErrorKind enum), `warning_kind.rs`
(TypeWarningKind enum), `display.rs` (Display impls for both).

### 20j-B12 — `corvid-runtime/src/store.rs` (988 → ~250 lines, 3 commits)

Extract `policy_parse.rs` (parse_*_policy fns), `sqlite_backend.rs`
(SqliteStoreBackend), `memory_backend.rs` (InMemoryStoreBackend).
Trait + manager stay in `store.rs`.

### 20j-B13 — `corvid-shadow-daemon/src/replay_pool.rs` (830 → ~250 lines, 3 commits)

Extract `executors.rs` (Interpreter + Native), `spec.rs` (typed
records), `parse.rs` (IR-parsing helper).

### 20j-B14 — `corvid-runtime/src/approver_bridge.rs` (760 → ~250 lines, 3 commits)

Extract `compile.rs` (Corvid-source compilation + ABI emission),
`state.rs` (state mgmt), `simulate.rs` (simulation harness).

### 20j-B15 — `corvid-types/src/effects/cost.rs` (744 → ~400 lines, 2 commits)

Extract `render.rs` (render_cost_tree + format_numeric_dimension)
and `query.rs` (cost_path_for_dimension + numeric_constraint_value).
`compute_worst_case_cost` stays.

## Slice 20j-C — Smaller-but-mixed (4 files, ~12 commits)

Quick wins. Each file is below 800 lines but fails criterion 2 or 1.

### 20j-C1 — `corvid-cli/src/replay.rs` (709 → ~100 lines, 3 commits)

Extract `plain.rs`, `differential.rs`, `mutation.rs`.

### 20j-C2 — `corvid-runtime/src/approvals.rs` (702 → ~250 lines, 3 commits)

Extract `card.rs` (ApprovalCard + ApprovalCardArgument + ApprovalRisk),
`token.rs` (ApprovalToken + ApprovalTokenScope), `approver_impls.rs`
(StdinApprover + ProgrammaticApprover). Trait + ApprovalRequest +
ApprovalDecision stay.

### 20j-C3 — `corvid-cli/src/observe_cmd.rs` (691 → ~100 lines, 3 commits)

Extract `list.rs`, `show.rs`, `drift.rs` (each holding its build_*
+ render_* pair).

### 20j-C4 — `corvid-cli/src/routing_report.rs` (676 → ~150 lines, 2 commits)

Extract `build.rs` (RoutingReport ingestion + git invocation) and
`render.rs` (text formatter). Records stay in mod.rs.

## Validation gate

Run between every commit, in order:

```bash
cargo check --workspace
cargo test -p <crate-being-modified> --lib
cargo run -q -p corvid-cli -- verify --corpus tests/corpus
```

Pass criteria:

1. `cargo check --workspace` reports zero new errors. The two
   pre-existing OTel deprecation warnings in
   `corvid-runtime/src/otel_sdk_export.rs` are tolerated.
2. Targeted lib tests pass for every modified crate. Any failure
   means the refactor commit changed semantics — bug-fix commit on
   a separate branch, not in the refactor stream.
3. `corvid verify --corpus tests/corpus` exits `1` only on the two
   deliberate fixtures (the corpus contains intentional negatives).
   The `whoami` linker error in `corvid-driver --lib` is environmental
   and not a phase-20j gate.

## Phase-done checklist

- [ ] Slice 20j-S complete (4 files, ~12 commits).
- [ ] Slice 20j-A complete (14 files, ~80 commits).
- [ ] Slice 20j-B complete (14 files, ~52 commits).
- [ ] Slice 20j-C complete (4 files, ~12 commits).
- [ ] Workspace re-audited; no `.rs` file ≥600 lines fails the
  rubric. Final pass uses the same agent prompt that found the
  initial 31 violations.
- [ ] `docs/phase-20j-refactor.md` updated with closing inventory:
  every entry above carries a "Result" column with the post-split
  line count and target-module list, mirroring 20i's
  audit-record format.
- [ ] `learnings.md` updated with the per-slice learnings, per the
  CLAUDE.md "Update learnings.md per user-visible slice" rule.
- [ ] ROADMAP.md's Phase 20j entry is checked.
- [ ] Memory record `project_phase_20j_closed.md` written summarising
  the rubric-failure patterns that grew back post-20i (regrowth
  vectors: runtime.rs's `impl Runtime` accretion, CLI command
  files bundling subcommands, codegen-cl lowering accretion).

## Sequencing reminder

Per CLAUDE.md "pre-phase chat mandatory" and "no autonomous
chaining": each slice (S, A, B, C) and each sub-slice within a
slice (e.g., 20j-A1, 20j-A2) gets its own pre-phase confirmation
before any file moves. Refactor commits land sequentially with
push between, never batched.
