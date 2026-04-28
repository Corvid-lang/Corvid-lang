# Phase 36 Backend Core

This document is the implementation brief for Phase 36. It defines the
backend surface before code changes land so later slices do not invent
syntax, runtime ownership, or acceptance criteria ad hoc.

Phase 36 exists to let Corvid build a production-shaped HTTP backend without
requiring a Rust, Python, Node, or Go host application. The backend layer must
carry the same AI-native contracts as the language core: effects, approvals,
budgets, provenance, replay, traces, model routing, signed claims, and useful
diagnostics.

## Product Goal

A developer can write this kind of service entirely in Corvid:

```corvid
type RefundRequest:
    order_id: String
    amount: Float
    reason: String

type RefundResponse:
    receipt_id: String
    status: String

effect transfer_money:
    cost: $0.05
    trust: human_required
    reversible: false

tool issue_refund(req: RefundRequest) -> RefundResponse dangerous uses transfer_money

@budget($0.25)
agent approve_refund(req: RefundRequest) -> RefundResponse uses transfer_money:
    approve IssueRefund(req.order_id)
    return issue_refund(req)

server refund_api:
    route POST "/refunds" body RefundRequest -> json RefundResponse:
        return approve_refund(body)
```

The production shape matters more than raw web-framework novelty. The service
must compile to a single runnable backend binary, expose typed routes, validate
JSON boundaries, emit request traces, support health/readiness, validate config,
and preserve signed contract metadata.

## Syntax Direction

Phase 36 introduces a top-level `server` declaration and nested `route`
declarations:

```corvid
server <name>:
    config:
        <field>: <type> = env("<NAME>")
        <field>: Option<T> = env_optional("<NAME>")

    middleware:
        cors
        request_log
        timeout 30s
        body_limit 1MB

    route GET "/orders/{id}" -> json Order:
        return get_order(path.id)

    route POST "/refunds" body RefundRequest -> json RefundResponse uses transfer_money:
        return approve_refund(body)
```

Route grammar target:

```text
server_decl := "server" ident ":" newline indent server_item+ dedent

server_item :=
      config_block
    | middleware_block
    | route_decl

route_decl :=
    "route" http_method string_path route_inputs? "->" response_kind type uses? ":" newline block

route_inputs :=
      "body" type
    | "query" type
    | "body" type "query" type

response_kind := "json" | "text" | "empty"
http_method   := "GET" | "POST" | "PUT" | "PATCH" | "DELETE"
```

The first implementation may accept a minimal subset for 36B, but it must keep
the AST extensible for the full form above.

## Built-In Route Values

Inside a route body, the compiler binds route-scoped values:

- `path`: a generated struct from `{param}` path captures.
- `query`: the declared query type, if present.
- `body`: the declared request body type, if present.
- `headers`: read-only header map.
- `request_id`: server-generated request id.
- `config`: typed server config object.

These are ordinary typed values. They must not bypass existing effect,
approval, budget, provenance, or replay checks.

## Runtime Ownership

The Corvid runtime owns:

- HTTP listener lifecycle.
- Request id generation.
- Per-request trace context.
- JSON decode/encode boundaries.
- Route dispatch.
- Panic/error isolation.
- Timeouts and body-size limits.
- Health/readiness/metrics state.
- Config/env validation and secret redaction.

The generated server binary owns:

- The compiled Corvid route functions.
- The embedded route manifest.
- The embedded ABI descriptor and signed attestation when signing is enabled.

The host environment owns:

- Port binding policy.
- TLS termination unless a later slice adds native TLS.
- Process supervision, container orchestration, log shipping, and key
  management.
- Provider credentials and secret rotation.

## Route Manifest

`corvid build --target=server` must eventually embed a route manifest beside
the ABI descriptor:

```json
{
  "schema_version": 1,
  "server": "refund_api",
  "routes": [
    {
      "method": "POST",
      "path": "/refunds",
      "request_body": "RefundRequest",
      "response": "RefundResponse",
      "effects": ["transfer_money"],
      "approval_required": true,
      "trace": true
    }
  ]
}
```

The manifest is not a replacement for the ABI descriptor. It is the backend
operational index: hosts, tests, docs, metrics, and health tooling can inspect
what the server exposes without parsing source.

## Error Model

Route failures must be typed and route-aware:

- JSON decode error: method, route path, request id, field path, expected type,
  received value kind.
- JSON encode error: route path, response type, field path.
- Route panic/trap: route path, request id, agent/function name when available.
- Timeout: route path, configured timeout, elapsed duration.
- Body too large: configured limit and actual size if known.
- Config missing/invalid: env var name, expected type, redacted value status.

Diagnostics produced at compile time must preserve source spans. Runtime errors
must preserve route names and request ids.

## AI-Native Contract Rules

Routes are not an escape hatch from Corvid's safety model.

- A route body that calls a dangerous tool still needs a reachable approval
  contract.
- Route `uses` rows must cover body effects just like agents.
- Route calls into agents/prompts/tools participate in budget and confidence
  composition.
- `Grounded<T>` response values require provenance.
- Request traces include method, route, status, duration, request id, and the
  effect profile.
- Signed server builds must include route/server claim metadata and must fail
  closed when a route-level contract has no registered guarantee.

## Non-Scope For Phase 36

These are intentionally not part of Phase 36:

- Native TLS termination. Use a reverse proxy or platform load balancer.
- HTTP/2 and WebSockets.
- Distributed multi-process routing or service mesh integration.
- Database persistence. Phase 37 owns persistence and migrations.
- Durable background jobs. Phase 38 owns job queues and schedulers.
- OAuth connector flows. Later phases own real connector products.
- Hot reload in production.
- Raw performance parity with Go on trivial hello-world routes.

Phase 36 must instead prove Corvid reduces AI-backend governance glue: typed
routes, effect-aware deploy checks, approval gates, traces, env validation, and
signed server claims in one path.

## Slice Acceptance Tests

### 36B Minimal Server Target

Implementation convention for 36B only: before `server` declarations land, the
server target accepts the existing native entrypoint shape (a single agent, or
an agent named `main`) and exposes it at `GET /`. The generated server also
serves `GET /healthz`. Later slices replace this convention with typed
`server` / `route` declarations without removing the target.

- `corvid build --target=server examples/backend/hello_server/main.cor`
  produces a runnable binary.
- Running the binary with `CORVID_PORT=0` prints the bound address.
- `GET /healthz` returns 200.
- `GET /` invokes the compiled Corvid entrypoint and returns a JSON response.
- Unsupported high-level server syntax fails with a clear diagnostic rather
  than silently compiling.

### 36C Typed Route Model

- Path captures are typed and available through `path`.
- Query structs decode from query params.
- Request body structs decode from JSON.
- A route returning the wrong response type fails at compile time.
- Duplicate method/path pairs fail at compile time.

Implementation convention for 36C: `server` and nested `route` are now real
AST/parser/resolver/typechecker constructs. Path captures from `{name}` are
typed as `String`; `query Type` and `body Type` bind route-local `query` and
`body` values; `-> json Type` sets the route return contract. Runtime
query/body JSON decoding lands in 36D with the server dispatch layer.

### 36D JSON Boundary

- Malformed requests produce route-aware 400 JSON errors.
- Unsupported methods produce route-aware 405 JSON errors.
- Handler/runtime failures produce route-aware 500 JSON errors.
- Error bodies are stable JSON with `request_id`, `route`, `kind`, and
  `message`, and response headers include the same request-id shape.

Implementation convention for 36D: the generated server wrapper owns the stable
HTTP JSON error envelope before full typed route dispatch exists. Field-level
decode errors for `route ... body Type` and response encode errors use this
same envelope when 36E+ move dispatch from the transitional root handler to the
typed route manifest.

### 36E Runtime Basics

- Request ids are unique per request and returned in a response header.
- Handler timeout produces a controlled 504.
- Body limit produces a controlled 413.
- Handler failures are isolated from the server process.
- `CORVID_MAX_REQUESTS` gives the generated single-process server a graceful
  drain-and-exit path for test and supervisor-controlled shutdown.

Implementation convention for 36E: the transitional generated server remains
std-only and synchronous, but the runtime boundary now has the production
failure modes later route dispatch will reuse: request IDs from a process-wide
counter/time pair, `CORVID_HANDLER_TIMEOUT_MS`, bounded request reads, stable
JSON 413/504 errors, and subprocess isolation for compiled Corvid handlers.

### 36F Route Tracing

- Every request emits start/end trace events.
- Trace events include method, route pattern, status, duration, request id,
  and effect metadata.
- Failed decode and timeout cases are traced.

### 36G Health, Readiness, Metrics

- `/healthz` reports process liveness.
- `/readyz` reports config/provider readiness.
- `/metrics` exposes request counts, error counts, and latency buckets.
- Generated endpoints cannot collide with user routes unless explicitly
  overridden with a compile-time warning or error.

### 36H Config And Secrets

- Required env vars are validated before the listener starts.
- Optional env vars have typed `Option<T>` values.
- Invalid typed env vars fail startup with redacted diagnostics.
- `corvid doctor` reports missing backend config without printing secrets.

### 36I Approval/Effect Integration

- A dangerous route path without approval fails before deploy.
- An imported dangerous tool keeps its approval requirement through route calls.
- Route manifests mark approval-required routes.
- Signed server builds refuse incomplete route contract claims.

### 36J Backend Example

- `examples/backend/refund_api` starts locally.
- Route tests cover happy path, bad JSON, missing approval, health/readiness,
  and trace emission.
- The example builds with `--target=server`.
- The example has a README showing local run, test, config, and signed-claim
  verification commands.

## Benchmark Posture

The Phase 36 benchmark compares the refund API against FastAPI,
Express/Fastify, and Go HTTP on:

- Lines of handwritten governance code.
- Number of separate libraries/config files needed for auth, validation,
  traces, approvals, and signed claims.
- Cold-start and steady request overhead excluding provider latency.
- Error quality for bad JSON and missing config.
- Ability to explain the deployed AI contract from an artifact.

Corvid does not need to beat Go on raw hello-world throughput in Phase 36.
It must beat library stacks on production AI-backend contract density.
