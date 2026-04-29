# Phase 41 Production Connectors

Phase 41 makes Corvid useful for production personal and enterprise agents by
shipping owned connector contracts for email, calendar, chat, tasks, and local
files. A connector is not just an HTTP wrapper. It is a typed backend boundary
with declared scopes, effects, data classes, approval requirements, rate limits,
mock/replay behavior, redaction rules, and trace evidence.

## Non-Scope

Phase 41 does not ship a hosted OAuth broker, hosted token vault, hosted
connector marketplace, or broad auto-generated SDK surface. Provider clients are
hand-owned around the workflows Corvid apps need first. Live-provider tests are
opt-in behind environment variables; mock and replay modes are mandatory in CI.

## Connector Manifest

Every connector ships a manifest with this shape:

```toml
schema = "corvid.connector.v1"
name = "gmail"
provider = "google"
mode = ["mock", "replay", "real"]

[[scope]]
id = "gmail.read_metadata"
provider_scope = "https://www.googleapis.com/auth/gmail.metadata"
data_classes = ["email_metadata"]
effects = ["network.read"]
approval = "none"

[[scope]]
id = "gmail.send"
provider_scope = "https://www.googleapis.com/auth/gmail.send"
data_classes = ["email_metadata", "email_body", "external_recipient"]
effects = ["network.write", "send_email"]
approval = "required"

[[rate_limit]]
key = "user_id"
limit = 250
window_ms = 1000
retry_after = "provider_header"

[[redaction]]
field = "message.body"
strategy = "hash_and_drop"

[[replay]]
operation = "send"
policy = "quarantine_write"
```

The manifest is a contract. Runtime code must reject undeclared scopes, missing
approval requirements for write effects, missing replay policy, missing
redaction for sensitive data classes, and rate-limit declarations that cannot be
enforced.

## Runtime Contract

The shared connector runtime owns:

- OAuth token state references, never raw token logging.
- PKCE state validation and refresh-token rotation hooks.
- Scope minimum checks before live calls.
- Per-tenant and per-user rate limits.
- Retry policy honoring provider `Retry-After` and bounded exponential fallback.
- Redaction before trace emission.
- Lineage events for every read, write, retry, approval, replay, and provider
  error.
- Mock, replay, and real mode selection through one interface.
- Webhook signature verification helpers for providers that support webhooks.

## Provider Surface

Phase 41 provider order:

- Gmail/Google Workspace: message metadata, search, draft, send with approval,
  labels, attachment metadata, OAuth refresh.
- Microsoft 365: Outlook mail, calendar basics, contacts, Graph auth,
  tenant-aware scopes.
- Calendar: availability, event read, create, update, cancel, reminders,
  approval-gated external invites.
- Slack: channel/DM metadata, thread reads, draft/send with approval,
  workspace/user scoping.
- Linear and GitHub: issue read/search, create, update, comment, approval-gated
  writes.
- Local files: indexed folder metadata, read permissions, write approval,
  provenance snippets.

## Mock And Replay

No connector ships without:

- A deterministic mock backend.
- Replay fixture loading.
- Tests that run the same connector contract in mock mode.
- Fixtures for provider errors, malformed JSON, 429/5xx retries, expired OAuth
  state, revoked refresh token, and write quarantine.

Replay mode must never execute provider writes. Write operations in replay mode
return recorded evidence or an explicit quarantine error.

## Approval Rules

All external writes require approval by default:

- Gmail send and label changes that affect external workflows.
- Outlook send, calendar external invite, Teams message.
- Slack message send.
- Linear/GitHub create, update, comment.
- Local file write, update, delete.

Read operations may still require approval when data classes are marked
restricted by the app policy.

## Trace Evidence

Connector lineage events must populate:

- `trace_id`, `span_id`, `parent_span_id`.
- `tenant_id`, `actor_id`, `request_id`.
- `effect_ids`, `data_classes`, `approval_id`.
- `replay_key`, `idempotency_key`.
- `provider`, `operation`, and provider record ID through stable names or
  fingerprints.
- `cost_usd`, `latency_ms`, `status`, retry count, and redaction policy hash.

Raw tokens, message bodies, document contents, and webhook secrets must not
appear in traces.

## AI-Assisted Commands

AI helpers are allowed only when deterministic evidence remains primary:

- `corvid connectors mock-fixture-gen <name>` may draft fixtures from a
  provider sample, but the generated fixture is committed JSON and tested.
- `corvid connectors scopes-min <source>` may suggest smaller scopes, but
  manifests and tests enforce the result.
- `corvid connectors fail-sim <name>` may generate adversarial failure cases,
  but the corpus is committed and replayed deterministically.
- `corvid connectors check --live` may narrate drift, but raw provider
  responses and signed connector manifests remain the evidence.

## Acceptance Criteria

Before a connector is marked production-ready:

- Manifest parser accepts valid contracts and rejects missing approvals,
  undeclared effects, missing redaction, invalid rate limits, and missing replay
  policy.
- Mock mode passes in CI.
- Replay mode proves writes are quarantined.
- Real mode is documented behind provider-specific environment variables.
- Connector lineage is visible through `corvid observe`.
- Dangerous writes require approval and preserve audit evidence.
- Scope escalation and cross-tenant access tests fail closed.
