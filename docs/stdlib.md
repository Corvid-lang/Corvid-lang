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
