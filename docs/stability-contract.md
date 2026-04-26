# Corvid Stability Contract

This document defines the launch stability contract for Corvid v1.0.

## SemVer surface

The following are part of the public compatibility promise:

- core syntax accepted by the parser
- typechecker behavior for shipped language features
- CLI command names and top-level subcommand structure
- public standard-library module names under `std/*`
- published benchmark archive formats under `benches/results/*/ratios.json`
- published bundle formats documented in [`bundle-format.md`](./bundle-format.md)

## Compatibility rules

- Patch releases may fix bugs, improve diagnostics, and tighten behavior only where the documented semantics already required the stricter result.
- Minor releases may add new syntax, stdlib APIs, CLI subcommands, and metadata fields in backward-compatible ways.
- Major releases may remove or rename user-facing language/CLI/stdlib surfaces.

## Explicit non-promises

The following are not frozen as compatibility guarantees:

- internal trace event counts or wording outside documented schemas
- exact benchmark numbers
- unpublished archive layouts under temporary smoke or debug sessions
- private crate/module APIs

## Launch-claim discipline

Any public claim about Corvid's safety, replay, grounding, packaging, WASM, or benchmark behavior must point to:

1. a runnable command,
2. a checked-in example or archive,
3. or a test.

See [`launch-claim-audit.md`](./launch-claim-audit.md).
