# Host Compliance

`22-H` defines a compliant Corvid replay host as one that can:

1. record host-originated events into the shared JSONL trace via `corvid_record_host_event(...)`
2. respect the typed return status from that call
3. replay a recorded trace by loading the embedded library and executing `corvid_call_agent(...)` with `CORVID_REPLAY_TRACE_PATH` set
4. reproduce the recorded result across host languages for the Corvid-controlled execution surface

## Shared Trace Rules

- Host events live in the same trace stream as runtime events.
- The event shape is the normal `TraceEvent` envelope with `kind: "host_event"`.
- Only the cdylib writes the trace file. Hosts submit events through the C ABI; they do not append sidecar files.

## `corvid_record_host_event`

Signature:

```c
CorvidHostEventStatus corvid_record_host_event(
    const char* name,
    const char* payload_json,
    size_t payload_len);
```

Required status handling:

- `CORVID_HOST_EVENT_OK`: event was recorded
- `CORVID_HOST_EVENT_BAD_JSON`: host supplied malformed JSON and must not assume the event was recorded
- `CORVID_HOST_EVENT_TRACE_DISABLED`: recording is off; hosts may skip further payload work on that path
- `CORVID_HOST_EVENT_RUNTIME_ERROR`: writer-side failure such as filesystem error

A compliant host must not silently discard the status return. The compliance tests include a malformed JSON call and fail hosts that treat it as fire-and-forget.

## Replay Rules

- Replay uses the embedded binary plus the recorded trace; it does not require source recompilation.
- Hosts set `CORVID_REPLAY_TRACE_PATH` to the recorded trace and then dispatch the recorded top-level agent through `corvid_call_agent(...)`.
- For deterministic replay of the Corvid-controlled surface, hosts also set `CORVID_DETERMINISTIC_SEED` from trace metadata.

Corvid v1 of this guarantee is intentionally scoped:

- covered: Corvid-controlled RNG, trace-seeded timestamps/run identity, recorded call substitution
- not claimed: adapter-internal retry jitter, opaque SDK scheduling, or other non-Corvid-controlled internals

## Capsule Manifest

A replay capsule bundles:

- the cdylib
- the embedded descriptor as JSON
- the JSONL trace
- a manifest carrying content hashes plus:
  - `runtime_version`
  - `compiler_version`
  - `trace_schema_version`
  - `descriptor_abi_version`
  - `deterministic_seed`

Replay should warn on version mismatch and fail only on actual schema or ABI incompatibility.

## Reference Compliance Tests

`crates/corvid-host-compliance-tests` provides the executable reference:

- record in C via `examples/cdylib_catalog_demo/host_c/capsule_host.c`
- replay in Python via `examples/cdylib_catalog_demo/host_py/replay_host.py`

Passing that suite demonstrates cross-language replay compatibility for the current Corvid C ABI.*** End Patch

