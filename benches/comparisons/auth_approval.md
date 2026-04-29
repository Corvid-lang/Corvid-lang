# Phase 39 — auth + approval flow side-by-side

## Headline

For an idiomatic identity-aware approval flow (sessions + API keys +
JWT verification + tenant isolation + role-gated approval contracts +
batch approval + audit log), Corvid declares the contract in the
language and rejects unreachable approvals at typecheck; the
Auth.js (Next.js), FastAPI dependencies, and Go middleware baselines
configure the same checks across separate libraries with explicit
glue.

## Reproduce

The Corvid implementation pattern lives in
`apps/refund_bot/corvid/refund_bot.cor` and
`crates/corvid-runtime/src/{auth,approval_queue,approval_policy}.rs`.
The line count uses the governance-lines counter on the same
`apps/refund_bot/` corpus.

```bash
python benches/moat/governance_lines/runner/count.py \
    --apps-dir benches/moat/governance_lines/apps \
    --out      benches/moat/governance_lines/RESULTS.md
```

## Side-by-side (sketch)

### Corvid

```corvid
auth my_api:
    sessions: cookie("__corvid_sess", secure, http_only, same_site: lax)
    api_keys: header("Authorization", scheme: bearer)
    jwt: verify_rs256(jwks_url: env("JWKS_URL"))
    csrf: double_submit("__corvid_csrf")

tenant Org { id: String, plan: Plan }
role Admin, Reviewer, Member
permission CanIssueRefund: Admin | Reviewer

@dangerous
@requires(permission: CanIssueRefund)
@approval(contract: RefundApproval)
tool issue_refund(actor: Actor, order_id: String, amount: Money) -> Receipt

approval RefundApproval:
    target: order_id
    cost_ceiling: $5000
    data: financial
    irreversible: true
    expires_in: 24h
    required_role: Admin
    policy { actor.role == Admin && amount < $100 }
    batch_with: same_tool, same_data_class, same_role
```

The compiler rejects: (a) any reachable path from a route or job to
`issue_refund` whose lexical scope lacks an `approve` token, (b)
any `approve` whose `required_role` does not cover every reachable
caller, (c) any cross-tenant reference of an Org-scoped record into
a different tenant's tool. The runtime supplies the JWT verification
(slice 39K) and the `corvid auth` / `corvid approvals` CLI surface
(slice 39L).

### Python (FastAPI dependencies + auth library + custom approval) — bounty-open

A FastAPI equivalent uses dependency-injection for the actor /
session / role; JWT verification with PyJWT or Authlib; a custom
`approval` Pydantic model + an in-application policy clause; an audit
log written by an explicit middleware. The reachability check
("does every dangerous tool have an approval boundary?") is not
performed by the type system. Submission lands under
`runs/python/`.

### TypeScript (Next.js + Auth.js + zod approval contracts) — bounty-open

Next.js + Auth.js handles sessions / OAuth / JWT verify; the
approval contract is a hand-rolled zod schema; the policy clause is
TypeScript code; the reachability check is not performed. Submission
lands under `runs/typescript/`.

### Go (chi/echo + middleware + custom approval) — bounty-open

The Go baseline uses `golang-jwt`, a custom session middleware, an
approval table, and a typed RBAC. The reachability check happens
only at runtime. Submission lands under `runs/go/`.

## Governance line count

| Implementation | Feature lines | Governance lines | Total | % governance |
|---|---|---|---|---|
| Corvid (`apps/refund_bot/corvid/`) | 18 | 9 | 27 | 33.3% |
| Python (FastAPI dependencies) | bounty-open | bounty-open | — | — |
| TypeScript (Auth.js + Next.js) | bounty-open | bounty-open | — | — |
| Go (chi + middleware) | bounty-open | bounty-open | — | — |

The Corvid count is the same row used for the durable-jobs
comparison because the underlying app is the same; the AUTH/APPROVAL
contribution is a subset of those 9 governance lines.

## What Corvid wins on

- **Reachability at typecheck**: a route or job that reaches a
  `@dangerous` tool without a matching `approve` boundary fails to
  compile (slice 39H, gated by `approval.confused_deputy_typecheck`
  in the registry).
- **Tenant isolation at typecheck**: a record owned by tenant A
  cannot be passed to a tool that writes back into tenant B
  without a typed boundary; the type system catches the
  confused-deputy class (registry row
  `tenant.cross_tenant_compile_error`).
- **Approval contract is a typed record**, not a runtime convention:
  target / cost_ceiling / data / expires_in / required_role / policy
  clause / batch_with are all language-level fields the runtime
  enforces.
- **JWT kid rotation, CSRF double-submit, OAuth PKCE** are
  registry-named guarantees with their own positive + adversarial
  tests, not application-layer checklist items.

## What Corvid does not claim

- **Auth.js's OAuth provider catalog** is broader; Corvid does not
  ship every IdP integration on day one.
- **Raw request throughput** is not the moat; the comparison is on
  governance line count and the ability to detect unreachable
  approvals statically.
- **The `bounty-open` cells above are not yet measured.** They
  remain `bounty-open` until a submission lands.
