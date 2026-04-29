# Phase 41 — connectors side-by-side

## Headline

For an idiomatic Gmail / Slack / GitHub connector that supports
read + search + draft + approval-gated send, OAuth state with PKCE
+ refresh, mock + replay + real modes sharing one typed surface,
manifest-declared scopes / rate limits / data classes / redaction,
and webhook signature verification, the Corvid manifest declares the
contract and the runtime enforces it; the raw-SDK Python and
TypeScript baselines hand-write each of those concerns per
connector.

## Reproduce

The Corvid connector implementations live under
`crates/corvid-connector-runtime/src/{auth,gmail,slack,tasks,calendar,files,manifest,rate_limit,runtime,trace,test_kit}.rs`.
Manifest definitions live in
`crates/corvid-connector-runtime/manifests/<connector>.toml`.

Real-provider mode is gated behind `CORVID_PROVIDER_LIVE=1` and
binds `reqwest` per audit-correction slice 41K; CI runs mock by
default and an opt-in matrix runs against recorded VCR cassettes
so signature / rate-limit / Retry-After paths are exercised
without provider keys.

```bash
corvid connectors list
corvid connectors check --live
corvid connectors run --mode=mock|replay|real
corvid connectors oauth init <provider>
corvid connectors verify-webhook --sig=<...>
```

The CLI surface above is gated by slice 41L; the webhook signature
verification by slice 41M.

## Side-by-side (sketch)

### Corvid

```corvid
import std.connectors.gmail as gmail
import std.connectors.slack as slack

connector gmail uses oauth2_token, network_effect:
    scopes: [gmail.modify, gmail.send]
    rate_limit: 250_per_user_per_second
    redact: message.body in traces
    webhook_signed_by: env("GMAIL_WEBHOOK_SECRET")

agent triage(user_id: String) -> Brief uses gmail.read_metadata, summary_effect:
    msgs: List<Grounded<Message>> = gmail.search(user_id, "is:unread newer_than:1d")
    return summarise(msgs)

@dangerous
agent send_brief(user_id: String, brief: String) uses gmail.send:
    approve SendBrief(user_id, brief)
    return gmail.send(user_id, brief)
```

The compiler rejects `send_brief` without `approve`; the runtime
honors the manifest's scope minimum, rate limit, and redaction
rules; replay mode quarantines outbound calls; webhook handlers
reject unsigned payloads. Registry rows:
`connector.scope_minimum_enforced`,
`connector.write_requires_approval`,
`connector.rate_limit_respects_provider`,
`connector.contract_drift_detected`,
`connector.webhook_signature_verified`,
`connector.replay_quarantine`.

### Python (raw `google-api-python-client` + `slack_sdk` + `PyGithub`) — bounty-open

The Python baseline uses the official SDKs directly; OAuth state
storage, PKCE, refresh-token rotation, scope minimization, rate
limiting, mock vs real-mode swap, and webhook signature verification
are all hand-written application code. Submission lands under
`runs/python/`.

### TypeScript (`googleapis` + `@slack/web-api` + `octokit/rest`) — bounty-open

The TypeScript baseline uses `googleapis`, `@slack/web-api`, and
`octokit/rest`. Same observation: OAuth + scope + rate-limit +
mock-vs-real + webhook verify are application code. Submission
lands under `runs/typescript/`.

## Governance line count and "time to write a new connector"

| Implementation | Governance lines per connector | Time-to-write a new connector |
|---|---|---|
| Corvid (`crates/corvid-connector-runtime/`) | manifest + ~80 LOC | ~1 day given the manifest format |
| Python (raw SDK use) | bounty-open | bounty-open |
| TypeScript (raw SDK use) | bounty-open | bounty-open |

Corvid's row counts the manifest as the surface where scope, rate
limit, data class, redaction, and approval policy live; the runtime
shared crate handles HTTP, retry, redaction, and trace event emission
once for every connector.

## What Corvid wins on

- **Manifest declares scope / rate-limit / redact / data-class**;
  the typed surface means a connector cannot use a scope its
  manifest does not declare, and trace events are redacted
  according to the manifest, not the application.
- **One typed surface for mock + replay + real**: the same call
  site tests in mock, replays from cassettes, and runs live;
  switching modes is a runtime flag, not a code rewrite.
- **Approval boundary at typecheck for write methods**: a route or
  job that calls `gmail.send` or `slack.post` without `approve`
  fails to compile.
- **Webhook signature verification is registry-named**:
  `connector.webhook_signature_verified` is a positive +
  adversarial test pair, not an application-layer checklist.

## What Corvid does not claim

- **Provider feature breadth**: the official SDKs cover more
  surface area than Corvid's hand-rolled clients on day one; the
  bounty window keeps the comparison honest as features land.
- **Webhook receiver mounting** still relies on the Phase 36
  generated server; a developer running a different HTTP host has
  to bridge the verifier explicitly.
- **The `bounty-open` cells above are not yet measured.**
