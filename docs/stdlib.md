# Corvid Standard Library

Phase 32 starts the standard library as ordinary Corvid source under `std/`.
The modules are intentionally small and effect-explicit so they can be imported,
audited, packaged, and eventually shipped through the same content-addressed
package path as user code.

## `std.ai`

`std/ai.cor` contains reusable AI application data envelopes and pure helpers:

- `AiMessage` plus `system_message`, `user_message`, and `assistant_message`
- `AiSession` plus `start_session` and `next_turn`
- `ToolResultEnvelope` plus `tool_ok` and `tool_error`
- `ModelRoute` plus `route_to`
- `StructuredValidation` plus `validation_ok` and `validation_error`
- `Confidence` plus `confidence`
- `TraceEventSummary` plus `trace_event`

These primitives are deliberately plain Corvid types and agents. A program can
import the module today with a local path:

```corvid
import "./std/ai" use AiMessage, user_message

agent main() -> String:
    msg = user_message("hello")
    return msg.content
```

Later Phase 32 slices will add package-style `std.ai` resolution and extend the
same module with routing, prompt rendering, structured-output validation, and
trace helpers that carry effects, replay, cost, and provenance metadata.

## `std.http`

`std/http.cor` defines request/response envelopes for typed HTTP workflows:

- `HttpHeader`
- `HttpRequestEnvelope` plus `http_get`, `http_post_json`, `http_with_retry`,
  and `http_with_timeout`
- `HttpResponseEnvelope` plus `http_ok`

The native runtime also exposes a matching `HttpClient`/`HttpRequest` API. Its
calls emit `std.http.request`, `std.http.response`, and `std.http.error` trace
events with method, URL, timeout, retry, status, attempt, latency, and payload
size metadata.

## `std.io`

`std/io.cor` defines path and file-system envelopes:

- `PathInfo`
- `FileReadEnvelope`
- `FileWriteEnvelope`
- `DirectoryEntryEnvelope`

The runtime exposes structured text read, text write, and directory listing
APIs. Each emits `std.io.*` trace events with operation, path, byte count,
entry count, latency, and error metadata.

## `std.secrets`

`std/secrets.cor` defines `SecretReadEnvelope` plus constructors for present and
missing reads. The runtime exposes environment-backed secret reads that return
the value to the caller but only emit redacted trace metadata:

- secret name
- whether the secret was present
- whether the value was redacted

Trace events never include the secret value.

## `std.observe`

`std/observe.cor` defines typed observability envelopes for metrics, cost
counters, latency histograms, routing decisions, approval summaries, and runtime
observation summaries.

The runtime exposes an observation snapshot API that aggregates normalized LLM
usage and provider health. Emitting the snapshot records a `std.observe.summary`
trace event with call counts, token totals, cost totals, local-call counts, and
degraded-provider counts.

## `std.cache`

`std/cache.cor` defines typed cache-key and cache-entry envelopes for prompt,
model, and tool-result caching. The runtime exposes deterministic cache-key
construction over namespace, subject, model, arguments, effect key, provenance
key, and version metadata. Cache-key creation emits `std.cache.key` trace events
so cache decisions are replay-auditable without storing cached payloads in the
metadata event.

## `std.queue`

`std/queue.cor` defines typed background-job envelopes with task, status, retry,
budget, effect-summary, and replay-key metadata. The runtime exposes an
in-process queue foundation for enqueue and cancel operations. Each operation
emits `std.queue.*` trace events so long-running AI work can be audited and later
backed by a durable store without changing the job contract.

## `std.agent`

`std/agent.cor` defines pure typed envelopes for common AI application patterns:
classification, extraction, ranking, adjudication, planning, tool-use records,
approval labels, critique/review, and grounded answer metadata. These are
ordinary Corvid values, so applications can compose them with effects, replay,
approval, provenance, and cache keys without introducing framework glue.
