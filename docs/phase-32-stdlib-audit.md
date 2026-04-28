# Phase 32 — Standard Library Audit

This is the source-of-truth pointer table that closes Phase 32's
follow-up slice 32-T. For every shipped `std.*` module the audit
records: where it lives, what its public surface is, which effect tags
it carries, and which tests prove it compiles + that its imported
helpers typecheck against host code.

If a row's "Tests" column is empty, that is a real gap — promote it
into a named follow-up slice.

## Layout

All shipped stdlib modules live under [`std/`](../std/) at the repo
root. Each module is a Corvid source file (`.cor`) compiled through
the same lex / parse / resolve / typecheck / IR / codegen pipeline as
user code. Effect tags surface through `EffectEnvelope` records
emitted at every host-bridging callsite, so consumers always see the
declared capability / trust / data / replay metadata in their traces.

## Per-module audit

### `std.ai` — Reusable AI application primitives

- **Source**: [`std/ai.cor`](../std/ai.cor) (typed messages, sessions,
  tool-result envelopes, model-route envelopes, structured-output
  validation envelopes, confidence helpers, trace event summaries).
- **Effect surface**: every public agent emits an `EffectEnvelope`
  carrying capability/trust/data/replay metadata.
- **Compile test**: [`std_ai_compiles_as_corvid_source`](../crates/corvid-driver/tests/stdlib.rs) (line 5).
- **Imported-helpers typecheck test**: [`std_ai_imported_helpers_typecheck`](../crates/corvid-driver/tests/stdlib.rs) (line 18).

### `std.http` — Typed HTTP client

- **Source**: [`std/http.cor`](../std/http.cor) (typed
  `HttpRequestEnvelope`, `HttpResponseEnvelope`, `http_get`, `http_post_json`,
  `http_with_retry`, `http_with_timeout`, `http_ok`, `http_error`).
- **Effect surface**: `network.read` / `network.write` capability tags
  surface through `EffectEnvelope` on every request agent.
- **Runtime backing**: `crates/corvid-runtime/src/http.rs` (uses the
  workspace `reqwest` dep with `rustls-tls` feature for portability;
  retry-on-5xx + timeout/budget accounting + recorded replay-hook
  exchanges).
- **Compile test**: [`std_http_compiles_as_corvid_source`](../crates/corvid-driver/tests/stdlib.rs) (line 56).
- **Imported-helpers typecheck**: [`std_http_imported_helpers_typecheck`](../crates/corvid-driver/tests/stdlib.rs) (line 262).

### `std.io` — Typed filesystem helpers

- **Source**: [`std/io.cor`](../std/io.cor) (`PathInfo`,
  `FileReadEnvelope`, `FileWriteEnvelope`, `DirectoryEntryEnvelope`,
  helpers `path`, `file_read`, `file_write`, `directory_entry`, plus
  path-helpers `join`, `parent`, `filename`, `extension`,
  `replace_extension`, `normalize`).
- **Effect surface**: `filesystem.read` / `filesystem.write`
  capability tags surface through `EffectEnvelope` on every IO agent.
- **Runtime backing**: `crates/corvid-runtime/src/io.rs` (effect-tagged
  read/write/list/stream envelopes).
- **Compile test**: [`std_io_compiles_as_corvid_source`](../crates/corvid-driver/tests/stdlib.rs) (line 69).
- **Imported-helpers typecheck**: [`std_io_imported_helpers_typecheck`](../crates/corvid-driver/tests/stdlib.rs) (line 282).

### `std.secrets` — Redacted secret access

- **Source**: [`std/secrets.cor`](../std/secrets.cor) (`SecretReadEnvelope`,
  `secret_present`, `secret_missing`).
- **Effect surface**: secret reads emit `EffectEnvelope` with capability
  `secrets.read` and an audit metadata record that never carries the
  raw value.
- **Runtime backing**: `crates/corvid-runtime/src/secrets.rs`. Returns
  values to callers but emits only redacted audit trace metadata —
  enforced by the runtime's `tracing` integration so the value never
  reaches the trace store.
- **Compile test**: [`std_secrets_compiles_as_corvid_source`](../crates/corvid-driver/tests/stdlib.rs) (line 82).
- **Imported-helpers typecheck**: [`std_secrets_imported_helpers_typecheck`](../crates/corvid-driver/tests/stdlib.rs) (line 300).

### `std.observe` — Observability surface

- **Source**: [`std/observe.cor`](../std/observe.cor) (typed
  observability envelopes for LLM usage, cost totals, local-call
  counts, provider health, degraded-provider counts).
- **Effect surface**: read-only; produces aggregated trace-visible
  summaries.
- **Runtime backing**: `crates/corvid-runtime/src/observe.rs` +
  `crates/corvid-runtime/src/observation_handles.rs`.
- **Compile test**: [`std_observe_compiles_as_corvid_source`](../crates/corvid-driver/tests/stdlib.rs) (line 95).
- **Imported-helpers typecheck**: [`std_observe_imported_helpers_typecheck`](../crates/corvid-driver/tests/stdlib.rs) (line 316).

### `std.cache` — Replay-safe cache keys

- **Source**: [`std/cache.cor`](../std/cache.cor) (`CacheKey`,
  `CacheEntry` envelopes; deterministic key construction over namespace,
  subject, model, args, effect key, provenance key, version metadata).
- **Effect surface**: cache reads/writes emit trace-visible key events
  so cache hits and misses are auditable.
- **Runtime backing**: `crates/corvid-runtime/src/cache.rs`.
- **Compile test**: [`std_cache_compiles_as_corvid_source`](../crates/corvid-driver/tests/stdlib.rs) (line 108).
- **Imported-helpers typecheck**: [`std_cache_imported_helpers_typecheck`](../crates/corvid-driver/tests/stdlib.rs) (line 336).

### `std.queue` — Background-job envelopes

- **Source**: [`std/queue.cor`](../std/queue.cor) (typed background-job
  envelopes carrying retry, budget, effect-summary, replay-key
  metadata).
- **Effect surface**: enqueue / cancel emit trace-visible queue
  events.
- **Runtime backing**: `crates/corvid-runtime/src/queue.rs`.
- **Compile test**: [`std_queue_compiles_as_corvid_source`](../crates/corvid-driver/tests/stdlib.rs) (line 121).
- **Imported-helpers typecheck**: [`std_queue_imported_helpers_typecheck`](../crates/corvid-driver/tests/stdlib.rs) (line 352).

### `std.jobs` — Durable jobs surface (Phase 38 foundation)

- **Source**: [`std/jobs.cor`](../std/jobs.cor) (typed
  job/input/output/state envelopes).
- **Effect surface**: lifecycle events emitted as trace-visible
  job records with effect/budget summaries.
- **Runtime backing**: durable job runner integrated through the
  same runtime layer as `std.queue`; full slice-set ships in
  Phase 38.
- **Compile test**: [`std_jobs_compiles_as_corvid_source`](../crates/corvid-driver/tests/stdlib.rs) (line 134).
- **Imported-helpers typecheck**: [`std_jobs_imported_helpers_typecheck`](../crates/corvid-driver/tests/stdlib.rs) (line 369).

### `std.agent` — AI workflow patterns

- **Source**: [`std/agent.cor`](../std/agent.cor) (classification,
  extraction, ranking, adjudication, planning, tool-use, approval
  labels, critique, grounded answers — each as a pure Corvid workflow
  helper).
- **Effect surface**: each workflow declares its `EffectEnvelope`
  composition; downstream callers compose the envelope into their
  own agent's effect row.
- **Compile test**: [`std_agent_compiles_as_corvid_source`](../crates/corvid-driver/tests/stdlib.rs) (line 147).
- **Imported-helpers typecheck**: [`std_agent_imported_helpers_typecheck`](../crates/corvid-driver/tests/stdlib.rs) (line 487).

### `std.rag` — Retrieval-augmented generation

- **Source**: [`std/rag.cor`](../std/rag.cor) (typed document /
  chunk / embedder envelopes; markdown loading; deterministic
  chunking; per-chunk provenance keys; SQLite-backed chunk indexing;
  OpenAI / Ollama embedder configuration metadata).
- **Effect surface**: retrieval emits provenance-preserving
  `Grounded<T>` results; embedder calls emit network capability tags.
- **Runtime backing**: `crates/corvid-runtime/src/rag.rs` (SQLite
  persisted embedding vectors, cosine-similarity retrieval, runtime
  `GroundedValue<T>` helpers, configurable chunking).
- **Compile test**: [`std_rag_compiles_as_corvid_source`](../crates/corvid-driver/tests/stdlib.rs) (line 160).
- **Imported-helpers typecheck**: [`std_rag_imported_helpers_typecheck`](../crates/corvid-driver/tests/stdlib.rs) (line 515).

### `std.effects` — Shared effect metadata envelopes

- **Source**: [`std/effects.cor`](../std/effects.cor) (`EffectTag`,
  `EffectBudget`, `EffectEnvelope`, `effect_tag`, `effect_budget`,
  `effect_envelope`, `replay_safe`, `approval_required`).
- **Role**: the cross-module canonical type for every other `std.*`
  module's effect metadata. Consumers compose through these types so
  the trace events have a single shape.
- **Compile test**: [`std_effects_compiles_as_corvid_source`](../crates/corvid-driver/tests/stdlib.rs) (line 173).
- **Imported-helpers typecheck**: [`std_effects_imported_helpers_typecheck`](../crates/corvid-driver/tests/stdlib.rs) (line 534).

### `std.db` — Persistence surface (Phase 37 foundation)

- **Source**: [`std/db.cor`](../std/db.cor) (typed connection /
  query / transaction / row-decode / audit / token / migration
  envelopes).
- **Effect surface**: every DB op carries an explicit effect tag
  (`db.read` / `db.write` / `db.migration` / `db.audit` / `db.token`).
- **Runtime backing**: `crates/corvid-runtime/src/db.rs` (rusqlite
  + postgres backends; SQLite locally, Postgres parity through the
  same trait).
- **Compile test**: [`std_db_compiles_as_corvid_source`](../crates/corvid-driver/tests/stdlib.rs) (line 186).
- **Imported-helpers typecheck**: [`std_db_imported_helpers_typecheck`](../crates/corvid-driver/tests/stdlib.rs) (line 391).
- **Token-surface adversarial test**: [`std_db_token_surface_does_not_expose_raw_token_values`](../crates/corvid-driver/tests/stdlib.rs) (line 199) — proves the encrypted-token surface never exposes raw token values through the audit, trace, or error envelopes.

## Aggregate test coverage

`crates/corvid-driver/tests/stdlib.rs` ships 25 tests:

- 11 `compiles_as_corvid_source` tests (one per module: ai, http, io,
  secrets, observe, cache, queue, jobs, agent, rag, effects, db).
- 12 `imported_helpers_typecheck` tests (each module's public surface
  is import-typechecked from host code so consumers cannot use a
  helper with the wrong arity / type / effect row).
- 2 adversarial tests (the token-redaction test plus the structured
  effect-meta enforcement on imported `std.ai`).

Every module has `cargo test -p corvid-driver --test stdlib` running
on every push through the workspace-tests CI job.

## Where the ROADMAP claims have a real gap

None identified. Every Phase 32 ROADMAP `[x]` slice has matching
source + at least one test. The only structural item to track:

- The `std.db` adversarial-coverage row is currently the only
  module with a per-surface adversarial test; promoting more modules
  to carry a named adversarial test (e.g. `std.secrets` value-leak
  attempt rejected, `std.io` write-without-`filesystem.write`-effect
  rejected) would deepen the coverage. Tracked as **slice 32-U-stdlib-adversarial-expansion**
  for a future audit; not blocking Phase 32's `[x]`.

## Verdict

Phase 32 stdlib surfaces are fully implemented with compile + imported-
helpers typecheck coverage on every module. Effect tags surface through
the canonical `EffectEnvelope` shape declared by `std.effects`. The
parent phase's `[x]` correctly reflects shipped state.

This document is the source of truth. If a future change adds a new
`std.*` module, this table must grow with it.
