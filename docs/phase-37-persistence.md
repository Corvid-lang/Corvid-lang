# Phase 37 Persistence, Migrations, And State

This document is the implementation brief for Phase 37. It defines the database
surface before code changes land so persistence does not become an ad hoc host
library bolted onto Corvid.

Phase 37 exists to let Corvid own durable backend state: explicit SQL queries,
typed row decoding, transactions, migrations, connector tokens, audit logs, and
effect-aware replay summaries for AI actions.

## Product Goal

A developer can write backend state like this in Corvid:

```corvid
type RefundRecord:
    id: String
    order_id: String
    amount: Float
    status: String

effect db_write:
    trust: internal
    reversible: true

agent record_refund(db: Db, refund: RefundRecord) -> RefundRecord uses db_write:
    std.db.execute(
        db,
        "insert into refunds (id, order_id, amount, status) values (?, ?, ?, ?)",
        [refund.id, refund.order_id, refund.amount, refund.status],
    )
    return refund
```

The posture is explicit SQL with Corvid-owned safety around it. Corvid should
not hide the database behind a magical ORM, but it should remove the production
glue developers normally hand-roll: migration checksums, drift detection,
typed decode errors, transaction boundaries, AI-action audit rows, effect tags,
and replay-safe summaries.

## SQL Posture

Phase 37 starts with SQLite and then adds Postgres parity for the subset needed
by reference apps.

- SQL remains explicit source text.
- Parameters are always passed separately from SQL text.
- Query result rows decode into declared Corvid record types.
- Decode diagnostics name query site, expected record type, missing column, and
  wrong value kind.
- Database calls are effect-tagged as reads or writes.
- Dangerous writes can be approval-gated through the existing approval system.

## Migration Model

Migrations are checked-in files, not generated hidden state.

```text
migrations/
  0001_init.sql
  0002_refund_audit.sql
```

`corvid migrate` owns:

- `up`: apply unapplied migrations in order.
- `down`: reverse only when a down migration exists.
- `status`: report applied, pending, missing, and drifted migrations.
- `--dry-run`: show what would run and fail CI on drift without mutating state.
- checksums: every applied migration records a content hash.

Drift is a hard failure. If a migration changed after being applied, Corvid
must report the file, expected checksum, actual checksum, and database record.

## Transaction Model

Transactions are explicit scopes.

- `transaction(db):` starts a transaction and commits on normal exit.
- A returned error or trap rolls back.
- Nested transactions are rejected in v1 unless a later slice adopts savepoints.
- Effects inside a transaction are summarized as one transaction event for
  replay, with per-query child summaries.

## Token And Secret Boundary

Connector tokens and OAuth refresh tokens need durable storage without making
Corvid a key-management system.

- Corvid provides encrypted token storage APIs.
- The host provides the encryption key through an explicit env/config boundary.
- `corvid doctor` validates that key presence and shape without printing it.
- Logs, traces, and audit rows store token references, never raw token values.

## AI-Native Audit Pattern

Phase 37 standardizes an audit-log schema for AI actions:

- actor and subject
- action and decision
- route/job/agent name
- prompt, model, and tool versions
- approval state
- effect row
- cost
- trace id
- replay key
- created timestamp

Audit writes are normal DB writes with first-class effects. A dangerous audit
path cannot silently bypass approval or replay policy.

## Non-Scope For Phase 37

- Hiding SQL behind a full ORM.
- Distributed transactions.
- Multi-primary replication.
- Online schema-change orchestration for very large databases.
- Native secret-manager integrations beyond the explicit host key boundary.
- Durable background job execution. Phase 38 owns queues and schedules.

## Slice Acceptance Tests

### 37A Persistence Design Brief

- This document defines SQL posture, migration rules, transaction rules,
  token-storage boundaries, audit-log shape, replay posture, and non-scope.

### 37B SQLite Connection Query

- `std.db` can open a SQLite database.
- Parameterized execute/query APIs reject raw interpolation shortcuts.
- Query errors include SQL site and database message.

Implementation convention for 37B: `std/db.cor` establishes the public Corvid
surface first: SQLite connection envelopes, parameter envelopes,
parameterized query/execute envelopes, result envelopes, redacted error
envelopes, and DB effect metadata. Host-backed execution will attach to this
surface in the next persistence slices.

### 37C Typed Row Decoding

- Query rows decode into declared records.
- Missing columns produce typed diagnostics.
- Wrong value kinds produce typed diagnostics.

### 37D Transactions

- Successful transaction scopes commit.
- Failed transaction scopes roll back.
- Nested transaction policy is enforced.

### 37E Migrations Drift

- `corvid migrate up/down/status` supports checksums.
- Dry runs report pending work.
- Drift fails with expected and actual checksum.

### 37F Audit Log Pattern

- Standard audit-log helpers record actor, action, prompt/model/tool versions,
  approval state, cost, trace id, and replay key.

### 37G Token Storage Boundary

- Token storage encrypts values with a host-supplied key.
- Diagnostics and traces redact token values.

### 37H Postgres Support

- Postgres supports the same query, decode, transaction, and migration subset
  needed by reference apps.

### 37I DB Effect Replay

- DB reads and writes carry effect tags.
- Replay records deterministic summaries without embedding raw secret data.

### 37J Backend State Example

- The backend example persists users, tasks, approvals, traces, connector
  tokens, and durable agent state through typed migrations and tests.
