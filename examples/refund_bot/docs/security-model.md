# Refund Bot Security Model

This document extends the canonical Corvid security model in
[`docs/security-model.md`](../../../docs/security-model.md). It does not add
new global guarantees; it maps the refund bot's app-specific threats to
existing Corvid guarantees and local tests.

## Trust Boundary

```text
operator seed data
    |
    v
RefundRequest -> approve_refund -> approve IssueRefund(req)
    |                                 |
    |                                 v
    +--------------------------> issue_refund(req) dangerous tool
                                      |
                                      v
                         RefundProviderResult typed surface
```

The app-specific trusted computing base is:

- Corvid parser, resolver, typechecker, approval checker, runtime, and replay
  engine as defined in the canonical security model.
- The `transfer_money` effect declaration in `src/main.cor`.
- The `issue_refund` tool signature and `approve_refund` agent body.
- The mock/replay/real adapter boundary that returns `RefundProviderResult`.
- Operator-controlled environment variables for real mode.

## Protected Assets

- Refund authorization intent: an operator must be able to distinguish an
  approved refund from an unapproved money-moving attempt.
- Refund provider credentials: `REFUND_PROVIDER_TOKEN` must never enter source,
  seed fixtures, traces, or CI logs.
- Replay fixtures: committed traces must stay deterministic and redacted.
- Audit identifiers: `audit_id` and `guarantee_id` must stay present on the
  typed refund provider surface.

## Named Threats

| Threat | Defense | Test |
| --- | --- | --- |
| `auth_bypass` | A direct call to `issue_refund` without `approve IssueRefund(...)` is rejected with `approval.dangerous_call_requires_token`. | `tests/adversarial/auth_bypass.cor` |
| `scope_escalation` | An escalated refund attempt still cannot call the dangerous tool without an approval token. | `tests/adversarial/scope_escalation.cor` |
| `replay_forgery` | A forged replay receipt string does not authorize the dangerous tool. | `tests/adversarial/replay_forgery.cor` |
| `prompt_injection_refund_reason` | User-controlled refund reasons are data; prompt-like text in the reason cannot bypass the approval requirement. | `tests/adversarial/prompt_injection_refund_reason.cor` |

`crates/corvid-cli/tests/demo_project_defaults.rs` typechecks each adversarial
fixture and asserts the registered `approval.dangerous_call_requires_token`
guarantee id.

## Replay Invariant

`tests/replay_invariant.cor` asserts that mock, replay, and real provider
entrypoints return the same `RefundProviderResult` fields for the deterministic
seed request. Mode selection is host configuration, not part of the typed
result surface.

## Non-Goals

- This demo does not prove provider-side fraud detection, chargeback handling,
  or payment processor settlement semantics.
- The current approval checker enforces the presence of a matching approval
  token; it does not prove semantic amount ceilings or per-field policy
  predicates inside `RefundRequest`.
- The demo is single-tenant. It does not claim tenant-crossing protection beyond
  keeping tenant-like seed metadata out of the Corvid refund request type.
- Real provider availability, rate limiting, and provider incident response are
  operator responsibilities documented in `runbook.md`.
