# CI Matrix

Corvid's default CI workflow lives in `.github/workflows/ci.yml`.

## `workspace-tests`

Runs the default Rust workspace checks without optional feature flags:

```text
cargo check --workspace
cargo test --workspace
```

This is the broad compile/test gate for the language platform.

## `python-features`

Runs the Python FFI runtime path that the default workspace tests do not cover:

```text
cargo test -p corvid-runtime --features python --tests -- --nocapture
```

The job pins CPython through `actions/setup-python` with `python-version: '3.11'`.
It verifies the PyO3 call bridge, GIL-bound execution, scalar/list/object
marshalling, traceback preservation, `python.call` / `python.result` /
`python.error` trace events, and sandbox-profile denials.

The job must stay separate from `workspace-tests` so default builds remain
Python-free while every push still exercises the optional Python integration.
