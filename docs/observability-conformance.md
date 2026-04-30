# OTel SDK conformance harness — slice 40J

Phase 40's audit-correction track (slice 40J) replaced the
hand-rolled JSON OTLP exporter with the standard
`opentelemetry` + `opentelemetry-otlp` SDK
(`crates/corvid-runtime/src/otel_sdk_export.rs`). The unit tests
in that module cover attribute construction, span-name shaping,
and the exporter's wire path against an in-process HTTP
receiver. This file is the live-receiver conformance harness:
how to verify, on a clean machine, that the SDK exporter reaches
a Jaeger collector and that the spans carry the
`corvid.guarantee_id`, `corvid.cost_usd`, `corvid.approval_id`,
`corvid.replay_key` attributes the Phase 40 phase-done checklist
names.

The harness is documented (not committed as a CI workflow)
because it requires Docker. The default CI runs the
`crates/corvid-runtime/src/otel_sdk_export.rs` unit tests; the
opt-in matrix below runs against a real Jaeger receiver.

## Prerequisites

- `docker` + `docker compose` available
- A free port at `localhost:4318` (OTLP/HTTP) and `localhost:16686`
  (Jaeger UI)

## docker-compose

```yaml
# docker-compose.observability.yml
services:
  jaeger:
    image: jaegertracing/all-in-one:1.64
    environment:
      - COLLECTOR_OTLP_ENABLED=true
    ports:
      - "4317:4317"   # OTLP/gRPC
      - "4318:4318"   # OTLP/HTTP
      - "16686:16686" # Jaeger UI
```

Save the YAML next to this doc, then:

```bash
docker compose -f docs/docker-compose.observability.yml up -d
```

## Step 1 — emit a span via the SDK

A short Rust program that wires the SDK exporter and pushes one
span lives at `examples/otel_sdk_smoke.rs` (planned;
intentionally not yet committed). Operators run it directly:

```bash
cargo run -p corvid-runtime --example otel_sdk_smoke
```

Or, equivalently, the test from the SDK module against the live
endpoint:

```bash
CORVID_OTEL_LIVE_ENDPOINT=http://localhost:4318/v1/traces \
  cargo test -p corvid-runtime --lib otel_sdk_export -- --ignored --nocapture
```

## Step 2 — verify the span shape in Jaeger

1. Open <http://localhost:16686> in a browser.
2. Service: `corvid-runtime-test` (the `service.name` attribute
   the SDK exporter sets via `Resource::new`).
3. Operation: `corvid.tool.search` (the span name pattern
   `corvid.<kind>.<event-name>` the exporter constructs).
4. Click the resulting span → the **Tags** panel must list at
   least:
   - `corvid.kind=tool`
   - `corvid.name=search`
   - `corvid.status=ok`
   - `corvid.guarantee_id=...`
   - `corvid.cost_usd=...`
   - `corvid.replay_key=...`

If every one of those keys appears, the SDK conformance bullet
in the Phase 40 phase-done checklist is satisfied. The unit
tests in `otel_sdk_export.rs::tests::span_attributes_*` already
assert that the attribute set is constructed with those keys
before the SDK serialises the span; this harness confirms the
SDK's wire format actually delivers them to a standard collector.

## What this harness does NOT prove

- That every Corvid runtime path emits OTel spans. The SDK
  exporter is one of two paths today (the hand-rolled
  `otel_export.rs` is the other). Production deployments install
  the SDK provider via `OtelSdkExporter::install_as_global`
  and then `tracing` events flow through the same pipeline; the
  switch is per-process.
- That the OTel collector queue has correct backpressure /
  retry behaviour under load. That's a property of the SDK and
  the collector, not Corvid; the upstream `opentelemetry-otlp`
  test suite covers it.

## Tearing down

```bash
docker compose -f docs/docker-compose.observability.yml down
```
