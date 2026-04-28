# Refund API Backend Example

This example is the Phase 36 production-shaped backend contract. It shows the
Corvid source shape for an approval-gated refund service:

- typed request and response records
- a dangerous money-transfer tool with an effect row
- a route-local approval path through `approve_refund`
- generated health, readiness, metrics, request IDs, JSON errors, traces, and
  backend env validation from the server target

Check the full approval-gated route contract:

```sh
corvid check src/refund_api.cor
```

Build the runnable generated server entrypoint:

```sh
corvid build src/main.cor --target=server
```

Run it:

```sh
CORVID_PORT=8080 ./target/server/main_server
```

On Windows:

```powershell
$env:CORVID_PORT = "8080"
.\target\server\main_server.exe
```

Current runtime boundary:

- `GET /healthz` returns liveness.
- `GET /readyz` returns readiness.
- `GET /metrics` returns runtime counters.
- `GET /` invokes the transitional compiled Corvid entrypoint.
- `src/refund_api.cor` contains the full typed `POST /refunds` contract with a
  dangerous money-transfer tool and approval gate.
- `src/main.cor` is the current runnable server entrypoint while route dispatch
  lowering is still being connected to `--target=server`.

Operational env checked by generated servers and `corvid doctor`:

- `CORVID_PORT`: integer port, `0` allowed for an OS-assigned port.
- `CORVID_HANDLER_TIMEOUT_MS`: unsigned integer timeout in milliseconds.
- `CORVID_MAX_REQUESTS`: positive unsigned integer drain-and-exit limit.

Invalid values fail with redacted diagnostics.
