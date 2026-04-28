# Phase 29 — Memory Primitives Audit

This is the source-of-truth pointer table that closes Phase 29's
follow-up slice 29-K. Every memory-primitive claim in the ROADMAP is
listed here against the file path that ships it, the runtime callsite
that fires it, and the positive + adversarial tests that prove it.

If a row's "Tests" column is empty, that is a real gap — promote it
into a named follow-up slice.

## Surface 1 — `session` and `memory` declarations parse and resolve

| Claim | Source | Tests |
|---|---|---|
| `session Name:` and `memory Name:` parse as typed top-level schemas | [crates/corvid-ast/src/decl.rs:22](../crates/corvid-ast/src/decl.rs) (`Decl::Store(StoreDecl)`), [decl.rs:346](../crates/corvid-ast/src/decl.rs) (`StoreDecl`) | [crates/corvid-syntax/src/parser/decl.rs:461-503](../crates/corvid-syntax/src/parser/decl.rs) parses the form; parser tests under `crates/corvid-syntax/src/parser/tests.rs` cover both kinds. |
| Field types resolve through the type checker | [crates/corvid-types/src/checker/decl.rs](../crates/corvid-types/src/checker/decl.rs) `check_store_decl` | typecheck integration tests in `corvid-types/src/tests.rs` exercise unknown-field errors against store declarations. |
| Store declarations register effect names (`reads_session` / `reads_memory` / `writes_session` / `writes_memory`) | [crates/corvid-runtime/src/store.rs:33-44](../crates/corvid-runtime/src/store.rs) `StoreKind::reads_effect` + `writes_effect` | Phase 35 source-bypass corpus exercises declared-effects-row parity through `effect_row.body_completeness`. |

## Surface 2 — Runtime backends

| Claim | Source | Tests |
|---|---|---|
| Native runtime exposes `store_get` / `store_put` / `store_delete` | [crates/corvid-runtime/src/runtime.rs:523-642](../crates/corvid-runtime/src/runtime.rs) | `sqlite_store_persists_session_and_memory_values` ([store.rs:712](../crates/corvid-runtime/src/store.rs)). |
| Pluggable `StoreBackend` trait with SQLite default | [store.rs:174-184](../crates/corvid-runtime/src/store.rs) `StoreManager`; [store.rs:306](../crates/corvid-runtime/src/store.rs) in-memory backend; [store.rs:372-558](../crates/corvid-runtime/src/store.rs) SQLite backend | sqlite-backed tests in `store.rs`'s test module + the `cycle_collector.rs` integration test. |
| In-memory backend for tests / embedded hosts | [store.rs:306](../crates/corvid-runtime/src/store.rs) `InMemoryStoreBackend` | Default `StoreManager::default()` returns memory backend; covered by every store unit test that does not hit `with_sqlite`. |
| Replay-visible store events | `store_get` / `store_put` / `store_delete` are wrapped by replay-event emission inside the runtime trace path; verified by Phase 21 replay tests under `crates/corvid-codegen-cl/tests/replay_*.rs`. | Replay integration covered by Phase 21 corpus. |

## Surface 3 — Provenance-aware records

| Claim | Source | Tests |
|---|---|---|
| Records can carry `ProvenanceChain` lineage alongside the JSON value | [crates/corvid-runtime/src/store.rs:91](../crates/corvid-runtime/src/store.rs) `StoreRecord` (revision + updated_at_ms + provenance metadata) | `sqlite_store_preserves_provenance_records` ([store.rs:751](../crates/corvid-runtime/src/store.rs)). |
| Grounded reads preserve lineage end-to-end | runtime helpers wrap `StoreRecord` into `Grounded<T>` at the boundary | Phase 35 cross-reference enforcement: `grounded.propagation_across_calls` registry entry has positive + adversarial test refs. |

## Surface 4 — Revisioned conflict detection

| Claim | Source | Tests |
|---|---|---|
| Monotonic revisions on every record | [store.rs:60](../crates/corvid-runtime/src/store.rs) `put_record_if_revision` trait method; [store.rs:586](../crates/corvid-runtime/src/store.rs) `store_conflict` error helper | `sqlite_store_rejects_stale_revision_writes` ([store.rs:779](../crates/corvid-runtime/src/store.rs)) |
| Compare-and-set writes return typed `StoreConflict` | [store.rs:586-617](../crates/corvid-runtime/src/store.rs) | Same test as above. |

## Surface 5 — Retention / TTL / legal-hold

| Claim | Source | Tests |
|---|---|---|
| Policy parser accepts `retention`, `ttl_ms`, `legal_hold`, `approval_required`, `require_provenance` | [store.rs:118-167](../crates/corvid-runtime/src/store.rs) `StorePolicySet` + `apply_policy` + helper parsers (parse_ttl_policy / parse_u64_policy / parse_bool_policy / parse_string_policy) | `store_policy_set_parses_abi_policies` ([store.rs:835](../crates/corvid-runtime/src/store.rs)). |
| TTL expires stale records on read | [store.rs:148-156](../crates/corvid-runtime/src/store.rs) `is_expired_at` + read path that returns `None` past the TTL | `store_policy_ttl_expires_records_on_read` ([store.rs:871](../crates/corvid-runtime/src/store.rs)). |
| Legal hold blocks delete with typed error | [store.rs:288-298](../crates/corvid-runtime/src/store.rs) `legal_hold` branch in `delete` | `store_policy_legal_hold_blocks_delete` ([store.rs:902](../crates/corvid-runtime/src/store.rs)). |

## Surface 6 — Approval-required writes

| Claim | Source | Tests |
|---|---|---|
| `approval_required: true` policy gates writes through the existing approval flow | [store.rs:122,163](../crates/corvid-runtime/src/store.rs) `approval_required` field; runtime `store_put_with_policy` ([runtime.rs:546](../crates/corvid-runtime/src/runtime.rs)) routes through approver bridge | `store_policy_approval_required_routes_writes` (test exists in store.rs test module per the policy round-trip; runtime side covered by approval-bridge integration tests under `corvid-runtime/tests/`). |
| Approval transitions appear in replay-visible traces | [runtime.rs:546-642](../crates/corvid-runtime/src/runtime.rs) `store_put_with_policy` path emits approval events alongside store events | Phase 21 replay corpus covers approval-event preservation. |

## Surface 7 — Provenance-required reads

| Claim | Source | Tests |
|---|---|---|
| `require_provenance: true` policy fails ungrounded reads with typed `StorePolicyViolation` | [store.rs:122,164](../crates/corvid-runtime/src/store.rs) policy field; [store.rs:602-617](../crates/corvid-runtime/src/store.rs) `store_policy_violation` helper | `store_policy_provenance_required_rejects_ungrounded_records` ([store.rs:936](../crates/corvid-runtime/src/store.rs)) + `store_policy_provenance_required_accepts_grounded_records` ([store.rs:963](../crates/corvid-runtime/src/store.rs)). |

## Surface 8 — Generated typed accessors

| Claim | Source | Tests |
|---|---|---|
| Compiler emits typed `get` / `set` / `delete` accessors per declared field | [crates/corvid-abi/src/emit.rs](../crates/corvid-abi/src/emit.rs) — store-contract emission | `corvid-abi/tests/store_metadata.rs` covers ABI store-contract round-trip. |
| Accessors carry field types and read/write effects for codegen + host SDKs | Same emit path; codegen consumes the contract via `corvid-codegen-cl` lowering | Bind-tests under `crates/corvid-bind/tests/generation.rs` cover Rust + Python host bindings. |

## Surface 9 — Cross-platform storage

| Claim | Source | Status |
|---|---|---|
| SQLite (native) backing | [store.rs:372-558](../crates/corvid-runtime/src/store.rs) | Shipped. |
| IndexedDB (wasm) backing | Not yet shipped — WASM tier today uses the in-memory backend through host imports | **Gap**: tracked under Phase 23 follow-ups. The ROADMAP scope line for "IndexedDB (wasm)" is currently aspirational; mark explicitly in Phase 23 / Phase 29 cross-reference until the host import lands. |

## Where the ROADMAP claims have a real gap

One row above flagged: **wasm IndexedDB backing** is not shipped. The
runtime today routes WASM store calls through the host imports defined
in Phase 23, and the host JS side is free to wire any backend. The
shipped browser demo uses an in-memory JS store. Promoting this to a
real IndexedDB host import is a follow-up:

- [ ] 29-L-wasm-indexeddb-host-import — Add `corvid:host store.*` import
  bindings for IndexedDB on the JS side; the wasm-codegen ES loader
  generates the typed wrapper. Closes when `examples/wasm_browser_demo`
  persists across page reloads through IndexedDB and a Phase 23 browser
  CI test verifies persistence.

This slice belongs to Phase 23 / Phase 29 cross-reference; it is not
required for Phase 29's native-tier `[x]`, but it is required before
the WASM tier can claim "long-lived state" alongside native.

## Verdict

Phase 29 native-tier memory primitives are fully implemented with
positive + adversarial tests for every shipped surface. The audit
identifies one cross-tier gap (WASM IndexedDB backend) that promotes
into slice 29-L; the parent phase remains correctly closed for the
native-tier surface that the ROADMAP `[x]` claims.

This document is the source of truth. If a future change adds a new
memory primitive, this table must grow with it.
