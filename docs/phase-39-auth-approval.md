# Phase 39 Auth, Identity, Tenant, and Approval Model

This document is the implementation contract for Phase 39. It defines what
Corvid must provide before auth and human approvals can be called production
ready. The goal is not to wrap a web framework. The goal is to make identity,
tenant isolation, permission checks, dangerous actions, and approval decisions
typed, auditable, replay-aware, and visible to the compiler/runtime.

External baseline references:

- OWASP Session Management Cheat Sheet:
  <https://cheatsheetseries.owasp.org/cheatsheets/Session_Management_Cheat_Sheet.html>
- OWASP CSRF Prevention Cheat Sheet:
  <https://cheatsheetseries.owasp.org/cheatsheets/Cross-Site_Request_Forgery_Prevention_Cheat_Sheet.html>
- NIST SP 800-63B:
  <https://pages.nist.gov/800-63-4/sp800-63b.html>

## Security Invariants

1. Every authenticated request resolves to exactly one `Actor`.
2. Every `Actor` belongs to exactly one active tenant context for a request,
   job, tool call, trace, and approval decision.
3. API keys and session secrets are never stored in plaintext.
4. Privilege changes rotate or invalidate sessions that could observe the old
   privilege boundary.
5. Dangerous tools cannot execute from a route, job, agent, or tool path unless
   a matching approval contract is reachable.
6. Approval tokens are tenant-bound, actor-bound, contract-bound, resource-bound,
   expiry-bound, and single-use.
7. Approval UI payloads are structured backend records, not trace text that a
   frontend has to parse.
8. Every auth and approval decision writes audit evidence with trace linkage.

## Identity Model

`std.auth` owns typed envelopes for:

- `Actor`: stable actor id, display label, actor kind, tenant id, role set,
  permission set, authentication method, assurance level, and trace id.
- `SessionRef`: opaque session id reference, actor id, tenant id, issued time,
  expiry, rotation counter, CSRF binding id, and redaction metadata.
- `ApiKeyRef`: opaque key id, service actor id, tenant id, scope set, expiry,
  last-used metadata, and hash algorithm.
- `JwtSubject`: issuer, subject, audience, tenant claim, key id, expiry, not
  before, algorithm, and verified scopes.
- `OAuthStateRef`: provider, tenant id, actor id, PKCE verifier reference,
  state fingerprint, nonce fingerprint, expiry, and replay key.

The raw session token, API key secret, OAuth refresh token, and JWT signing key
are outside the Corvid source surface. Corvid code can only carry references,
hashes, fingerprints, expiry metadata, and provider names.

## Tenant And Permission Model

Phase 39 introduces a shared authorization vocabulary:

- `Tenant`: organization/workspace id, plan, lifecycle state, and data boundary.
- `Role`: named tenant-local role.
- `Permission`: named capability that can be required by routes, tools, jobs,
  and approval contracts.
- `ActorPermissionSet`: resolved permissions for an actor in one tenant.

Permission checks are explicit values. A function that needs `CanIssueRefund`
does not accept a raw `String` user id; it accepts an `Actor` or an
`AuthorizationDecision` that proves the actor had that permission in the same
tenant and trace.

## Session Auth

Session auth must support:

- secure, HttpOnly, SameSite cookie defaults
- server-side session records
- idle timeout and absolute timeout
- rotation on login, privilege change, tenant switch, and recovery
- CSRF binding for unsafe HTTP methods
- trace propagation into route, job, tool, approval, and DB audit records

Cookies are bearer transport only. The server-side session record is the
authority.

## API Key Auth

API keys are service-account credentials:

- generated once and shown once
- stored as Argon2id or stronger password-hash records, never plaintext
- scoped to tenant, route family, job family, tool family, or explicit
  permission names
- revocable without code deployment
- redacted in diagnostics, traces, and exported fixtures

API keys resolve to service actors. A service actor cannot approve its own
dangerous action unless an explicit approval contract permits service approval.

## JWT And OAuth

JWT verification must be declarative and auditable:

- supported algorithms: RS256, ES256, EdDSA
- `alg=none` and algorithm downgrade are rejected
- `kid` rotation and JWKS cache metadata are visible in diagnostics
- issuer, audience, expiry, not-before, and clock skew are checked
- tenant and subject claims are mapped into typed envelopes

OAuth callbacks must require PKCE and a replay-protected state record. The
runtime stores token references, ciphertext hashes, provider, scopes, expiry,
and replay keys. Raw provider tokens are never exposed to Corvid source.

## CSRF Model

Browser session auth must protect unsafe methods with a signed double-submit or
synchronizer-token strategy. Corvid's default is:

- session-bound CSRF token id
- HMAC-signed token value
- constant-time token comparison
- required token on POST, PUT, PATCH, and DELETE unless the route is explicitly
  API-key/JWT-only
- audit event on missing, expired, or invalid token

SameSite cookies are defense-in-depth, not the only CSRF control.

## Approval Contract Model

An approval contract is a typed record generated from or attached to a dangerous
tool/action:

- contract id and version
- expected action
- tenant id
- target resource kind and id
- data classification touched
- max cost or max money movement
- irreversible flag
- expiry
- required role or permission
- requester actor id
- approver actor id once decided
- decision state
- reason/comment/delegation chain
- trace id, replay key, and idempotency key

Contracts are not free-form strings. A contract for `IssueRefund(order_id,
amount)` cannot approve `SendEmail(thread_id)` and cannot cross tenants.

## Approval Queue API

The approval API must support:

- create
- list by tenant, actor, contract, state, resource, and expiry
- inspect
- approve
- deny
- expire
- comment
- delegate
- audit export

All transitions are single-transaction state changes. Approve/deny/expire is
idempotent by approval id plus expected current state. Stale approvals fail
closed.

## UI Payload Contract

The backend exposes a stable approval payload schema:

- human title and summary
- action, target, requester, required approver role
- risk level, data touched, irreversible flag, expiry
- cost/money ceiling
- before/after fields when available
- trace and replay references
- allowed transitions for the viewing actor
- redaction state

Frontend code must be able to render the card without parsing traces or Corvid
source.

## Compiler And Runtime Enforcement

The compiler/runtime split is:

- Compiler: sees dangerous tools, approval annotations, required permissions,
  effect rows, route/job/tool reachability, and missing approval contracts.
- Runtime: verifies actor identity, tenant equality, expiry, single-use
  approval state, role/permission membership, CSRF/JWT/session facts, and audit
  writes.

Both layers are required. Static visibility without runtime verification is not
production auth. Runtime checks without static reachability are not Corvid's
value proposition.

## Threat Cases

The adversarial suite for Phase 39 must include:

- confused-deputy approval reuse
- approval created in one tenant and used in another
- stale approval replay after expiry
- session fixation across login
- API key scope escalation
- JWT `kid` downgrade or unknown-key fallback
- OAuth state tampering
- missing CSRF on unsafe browser route
- service actor approving its own dangerous action without policy
- batch approval drift across action, role, data class, or resource kind
- privilege escalation after role removal

## Non-Scope

Phase 39 does not build a frontend UI, an identity provider, or a hosted auth
service. It builds the backend language/runtime primitives that make those
systems safe to connect and hard to bypass.
