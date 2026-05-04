# Support Escalation Bot Security Model

This document extends the canonical Corvid security model in
[`docs/security-model.md`](../../../docs/security-model.md). It does not add
new global guarantees; it maps the support escalation bot's app-specific
threats to existing Corvid guarantees and local tests.

## Trust Boundary

```text
operator seed data
    |
    v
order id -> lookup_order tool -> Order
    |                         |
    |                         +-> escalate_to_human tool -> SupportOutcome
    |
    +-> approve IssueRefund(order, amount)
                          |
                          v
                 issue_refund dangerous tool -> SupportOutcome
```

The app-specific trusted computing base is:

- Corvid parser, resolver, typechecker, approval checker, runtime, and replay
  engine as defined in the canonical security model.
- The `lookup_order`, `escalate_to_human`, and `issue_refund` tool signatures
  in `src/main.cor`.
- The `IssueRefund` approval site before the dangerous refund tool call.
- The shared tool surface used by mock, replay, order DB, refund provider, and
  Slack escalation modes.
- The `SupportOutcome` return shape and the mock/replay/real entrypoints in
  `src/main.cor`.
- Operator-controlled environment variables for real mode.

## Protected Assets

- Refund authorization intent: an operator must be able to distinguish a human
  escalation from an approved money-moving action.
- Customer and order data: order ids, customer ids, and totals must not be
  rewritten by prompt-like support reasons or replay receipts.
- Refund provider credentials and Slack webhook URLs: secrets must never enter
  source, seed fixtures, traces, or CI logs.
- Replay fixtures: committed traces must stay deterministic and redacted, and
  the approval-denied trace must continue to fail closed.
- Audit identifiers: escalation ticket ids and refund audit ids must stay on
  the app-facing `SupportOutcome` surface.

## Named Threats

| Threat | Defense | Test |
| --- | --- | --- |
| `auth_bypass` | A direct call to `issue_refund` without `approve IssueRefund(...)` is rejected with `approval.dangerous_call_requires_token`. | `tests/adversarial/auth_bypass.cor` |
| `scope_escalation` | A refund amount changed outside an approval site still cannot call the dangerous tool without an approval token. | `tests/adversarial/scope_escalation.cor` |
| `replay_forgery` | A forged replay audit id does not authorize the dangerous refund tool. | `tests/adversarial/replay_forgery.cor` |
| `prompt_injection_support_reason` | User-controlled support reasons are data; instruction-like text in the reason cannot bypass the approval requirement. | `tests/adversarial/prompt_injection_support_reason.cor` |
| `tenant_crossing` | A refund for an order with a different `customer_id` still cannot call the dangerous tool without an approval token. | `tests/adversarial/tenant_crossing.cor` |

`crates/corvid-cli/tests/demo_project_defaults.rs` typechecks each adversarial
fixture and asserts the registered `approval.dangerous_call_requires_token`
guarantee id.

## Replay Invariant

`tests/replay_invariant.cor` asserts that mock, replay, and real support
entrypoints return the same `SupportOutcome` fields for the deterministic
escalation seed path. Mode selection is host configuration, not part of the
typed result surface. Provider-specific request ids, latency, and raw payloads
belong in redacted trace metadata or provider logs.

## Non-Goals

- This demo does not prove semantic refund amount ceilings or per-field
  approval predicates. It enforces the compiler-visible approval boundary
  before the dangerous refund tool call.
- This demo does not prove provider-side fraud detection, Slack delivery, or
  refund settlement semantics.
- Tenant crossing is modeled through the current `customer_id` seed data. A
  production multi-tenant auth policy would need an app-specific authorization
  checker beyond this demo's approval boundary.
- Real provider availability, rate limiting, outage handling, and provider
  incident response are operator responsibilities documented in `runbook.md`.
