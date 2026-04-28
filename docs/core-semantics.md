# Corvid core semantics

> Auto-generated from `corvid_guarantees::GUARANTEE_REGISTRY`. **Do not hand-edit.** Update by running
> `cargo run -q -p corvid-cli -- contract regen-doc docs/core-semantics.md` and committing the result.

Every public Corvid promise about effects, approvals, grounding, budgets, confidence, replay, provenance, and the ABI surface is enumerated below. Each row carries:

- a stable **id** referenced by diagnostics, tests, the bilateral verifier, and `corvid claim --explain`,
- a **kind** (which moat dimension it belongs to),
- a **class** — `static` (compiler refuses to produce a binary on violation), `runtime_checked` (runtime detects and surfaces), or `out_of_scope` (a documented promise that does NOT have a check today; reason recorded inline below),
- the pipeline **phase** that owns the enforcement.

Per the no-shortcuts rule, every `out_of_scope` entry carries an explicit reason. Anything Corvid does not defend appears below in plain language; we do not rely on omission.

## Summary

| id | kind | class | phase |
|----|------|-------|-------|
| `approval.dangerous_call_requires_token` | approval | static | typecheck |
| `approval.token_lexical_only` | approval | static | typecheck |
| `approval.dangerous_marker_preserved` | approval | static | resolve |
| `effect_row.body_completeness` | effect_row | static | typecheck |
| `effect_row.caller_propagation` | effect_row | static | typecheck |
| `effect_row.import_boundary` | effect_row | static | resolve |
| `grounded.provenance_required` | grounded | static | typecheck |
| `grounded.propagation_across_calls` | grounded | static | typecheck |
| `budget.compile_time_ceiling` | budget | static | typecheck |
| `budget.runtime_termination` | budget | out_of_scope | runtime |
| `confidence.min_threshold` | confidence | static | typecheck |
| `replay.deterministic_pure_path` | replay | runtime_checked | runtime |
| `replay.trace_signature` | replay | runtime_checked | runtime |
| `provenance_trace.receipt_signature` | provenance_trace | runtime_checked | runtime |
| `abi_descriptor.cdylib_emission` | abi_descriptor | static | codegen |
| `abi_descriptor.byte_determinism` | abi_descriptor | static | codegen |
| `abi_attestation.envelope_signature` | abi_attestation | runtime_checked | abi_verify |
| `abi_attestation.descriptor_match` | abi_attestation | runtime_checked | abi_verify |
| `abi_attestation.absent_reports_unsigned` | abi_attestation | runtime_checked | abi_verify |
| `platform.host_kernel_compromise` | platform | out_of_scope | platform |
| `platform.signing_key_compromise` | platform | out_of_scope | platform |
| `platform.toolchain_compromise` | platform | out_of_scope | platform |

## Detail

### Approval boundaries

#### `approval.dangerous_call_requires_token`
- **class**: static
- **phase**: typecheck

A call site invoking a `@dangerous` tool must have an `approve` token lexically in scope; otherwise the typechecker rejects the program.

#### `approval.token_lexical_only`
- **class**: static
- **phase**: typecheck

Approval tokens are lexically scoped — they cannot be returned, stored in fields, or passed across opaque boundaries to unlock a call site outside the original `approve` block.

#### `approval.dangerous_marker_preserved`
- **class**: static
- **phase**: resolve

A `@dangerous` marker cannot be erased by re-exporting or aliasing the tool through another module — every public alias preserves the original danger annotation.

### Effect rows

#### `effect_row.body_completeness`
- **class**: static
- **phase**: typecheck

A function's declared effect row must cover every effect actually produced by its body (including effects of called functions); under-reporting is a compile error.

#### `effect_row.caller_propagation`
- **class**: static
- **phase**: typecheck

Callers inherit the union of their callees' effects unless they declare a wider row; callers cannot silently shrink the effect surface.

#### `effect_row.import_boundary`
- **class**: static
- **phase**: resolve

Cross-module imports preserve effect annotations exactly; an importer cannot use a re-exported function with a stripped or weakened effect row.

### Grounded provenance

#### `grounded.provenance_required`
- **class**: static
- **phase**: typecheck

Constructing a `Grounded<T>` value requires citing a source; unsourced `Grounded` construction is a compile error.

#### `grounded.propagation_across_calls`
- **class**: static
- **phase**: typecheck

Provenance is preserved across function boundaries — a `Grounded<T>` returned from a callee retains its citation chain into the caller without separate annotation.

### Budgets

#### `budget.compile_time_ceiling`
- **class**: static
- **phase**: typecheck

An agent annotated `@budget($X)` fails compile if the sum of statically known per-call costs along any reachable path exceeds `$X`.

#### `budget.runtime_termination`
- **class**: out_of_scope
- **phase**: runtime

Live runtime termination of an agent when actual runtime cost crosses the `@budget($X)` threshold mid-execution.

> **Why out of scope:** Today Corvid enforces budgets at compile time via `budget.compile_time_ceiling`, and the runtime observes per-call cost in trace events; live mid-execution termination on threshold crossing is not yet implemented. A future slice can promote this entry back to `RuntimeChecked` once the enforcement ships. The compile-time ceiling is the load-bearing guarantee for v1.0.

### Confidence thresholds

#### `confidence.min_threshold`
- **class**: static
- **phase**: typecheck

An agent annotated `@min_confidence(X)` requires every input carrying a confidence tag to meet `X`; lower-confidence inputs are rejected at the call site.

### Replay determinism

#### `replay.deterministic_pure_path`
- **class**: runtime_checked
- **phase**: runtime

A trace recorded from a `@replayable` agent reproduces deterministically across `corvid replay` invocations on the same compiled binary; non-deterministic divergence raises the documented replay-divergence error.

#### `replay.trace_signature`
- **class**: runtime_checked
- **phase**: runtime

Trace receipts produced with `--sign` carry a DSSE envelope whose signature `corvid receipt verify` checks against the supplied verifying key.

### Provenance traces

#### `provenance_trace.receipt_signature`
- **class**: runtime_checked
- **phase**: runtime

`corvid receipt verify` rejects any DSSE-wrapped receipt whose signature does not validate against the supplied verifying key, with a non-zero exit and the documented `verification failed` message.

### ABI descriptor

#### `abi_descriptor.cdylib_emission`
- **class**: static
- **phase**: codegen

Every `corvid build --target=cdylib` output exports a `CORVID_ABI_DESCRIPTOR` symbol whose payload is the canonical effect/approval/provenance surface for the compiled program.

#### `abi_descriptor.byte_determinism`
- **class**: static
- **phase**: codegen

Two byte-identical Corvid sources compiled with the same toolchain version produce byte-identical descriptor JSON; the descriptor is canonical, not pretty-printed.

### ABI attestation

#### `abi_attestation.envelope_signature`
- **class**: runtime_checked
- **phase**: abi_verify

`corvid receipt verify-abi` rejects a signed cdylib whose attestation envelope does not validate against the supplied verifying key, exiting 1 with `attestation verification failed`.

#### `abi_attestation.descriptor_match`
- **class**: runtime_checked
- **phase**: abi_verify

After signature validation, the recovered attestation payload must bit-match the embedded `CORVID_ABI_DESCRIPTOR`; mismatch is rejected even when the signature is valid.

#### `abi_attestation.absent_reports_unsigned`
- **class**: runtime_checked
- **phase**: abi_verify

`corvid receipt verify-abi` on a cdylib lacking the `CORVID_ABI_ATTESTATION` symbol exits 2 with the documented `unsigned` message, leaving the host policy to decide whether to accept it.

### Platform — explicit non-defenses

#### `platform.host_kernel_compromise`
- **class**: out_of_scope
- **phase**: platform

Defending against a compromised host kernel or privileged-process tampering with the running Corvid binary's memory.

> **Why out of scope:** Outside Corvid's trust boundary — a kernel that can rewrite user-space memory can defeat any user-space invariant. The security model assumes a non-malicious kernel; otherwise the host is responsible.

#### `platform.signing_key_compromise`
- **class**: out_of_scope
- **phase**: platform

Defending against compromise of the ed25519 signing key used to attest a cdylib or sign a receipt.

> **Why out of scope:** Key management is a host responsibility. Corvid signs and verifies; rotating, revoking, and protecting keys is outside the language's scope and explicitly delegated to the host's key-management practice.

#### `platform.toolchain_compromise`
- **class**: out_of_scope
- **phase**: platform

Defending against a compromised Rust toolchain, Cranelift release, or system linker producing a Corvid binary that does not match its source.

> **Why out of scope:** Reproducible builds across heterogeneous toolchains are a post-v1.0 hardening goal. Today Corvid trusts the rustc and Cranelift releases the user installs; the bilateral verifier (Slice 35-H) is the closest approximation of toolchain-independence available pre-v1.0.

## Updating this document

This file is generated. To change a description, add a new guarantee, or move an entry between `static` /
`runtime_checked` / `out_of_scope`, edit `crates/corvid-guarantees/src/lib.rs` and run:

```
cargo run -q -p corvid-cli -- contract regen-doc docs/core-semantics.md
```

Then commit the regenerated file together with the registry change. CI fails if the committed text drifts from the registry — there is no quiet way to evolve the spec away from the implementation.
