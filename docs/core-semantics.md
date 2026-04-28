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
| `approval.dangerous_marker_preserved` | approval | static | typecheck |
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
| `abi_descriptor.bilateral_source_match` | abi_descriptor | runtime_checked | abi_verify |
| `abi_attestation.envelope_signature` | abi_attestation | runtime_checked | abi_verify |
| `abi_attestation.descriptor_match` | abi_attestation | runtime_checked | abi_verify |
| `abi_attestation.absent_reports_unsigned` | abi_attestation | runtime_checked | abi_verify |
| `abi_attestation.sign_requires_claim_coverage` | abi_attestation | static | codegen |
| `platform.host_kernel_compromise` | platform | out_of_scope | platform |
| `platform.signing_key_compromise` | platform | out_of_scope | platform |
| `platform.toolchain_compromise` | platform | out_of_scope | platform |

## Detail

### Approval boundaries

#### `approval.dangerous_call_requires_token`
- **class**: static
- **phase**: typecheck

A call site invoking a `@dangerous` tool must have an `approve` token lexically in scope; otherwise the typechecker rejects the program.

**Positive tests:**

- `crates/corvid-types/src/tests.rs::dangerous_tool_with_matching_approve_is_ok`

**Adversarial tests:**

- `crates/corvid-types/src/tests.rs::dangerous_tool_without_approve_is_compile_error`
- `crates/corvid-types/src/tests.rs::tagged_unapproved_dangerous_call_carries_approval_guarantee_id`

#### `approval.token_lexical_only`
- **class**: static
- **phase**: typecheck

Approval tokens are lexically scoped — they cannot be returned, stored in fields, or passed across opaque boundaries to unlock a call site outside the original `approve` block.

**Positive tests:**

- `crates/corvid-types/src/tests.rs::outer_approve_authorizes_inner_call`

**Adversarial tests:**

- `crates/corvid-types/src/tests.rs::approve_does_not_leak_out_of_if_branch`
- `crates/corvid-types/src/tests.rs::mutation_nested_inner_approve_does_not_authorize_outer_call`

#### `approval.dangerous_marker_preserved`
- **class**: static
- **phase**: typecheck

A `@dangerous` marker cannot be erased by re-exporting or aliasing the tool through another module — every public alias preserves the original danger annotation.

**Positive tests:**

- `crates/corvid-types/tests/source_bypass_corpus.rs::baseline_for_alias_compiles_clean`

**Adversarial tests:**

- `crates/corvid-types/src/tests.rs::adversarial_source_mutator_import_use_alias_dangerous_tool_is_tagged`
- `crates/corvid-types/tests/source_bypass_corpus.rs::mutator_drops_approve_through_mock_alias_triggers_token_guarantee`

### Effect rows

#### `effect_row.body_completeness`
- **class**: static
- **phase**: typecheck

A function's declared effect row must cover every effect actually produced by its body (including effects of called functions); under-reporting is a compile error.

**Positive tests:**

- `crates/corvid-types/src/tests.rs::mutation_tool_uses_declared_effect_is_ok`

**Adversarial tests:**

- `crates/corvid-types/src/tests.rs::mutation_baseline_trust_violation_exists`
- `crates/corvid-types/src/tests.rs::mutation_multiple_effects_on_one_tool_compose_cost_and_trust`

#### `effect_row.caller_propagation`
- **class**: static
- **phase**: typecheck

Callers inherit the union of their callees' effects unless they declare a wider row; callers cannot silently shrink the effect surface.

**Positive tests:**

- `crates/corvid-types/src/tests.rs::sub_agent_costs_propagate_into_outer_agent`

**Adversarial tests:**

- `crates/corvid-types/src/tests.rs::mutation_inner_agent_effects_propagate_to_outer_agent`

#### `effect_row.import_boundary`
- **class**: static
- **phase**: resolve

Cross-module imports preserve effect annotations exactly; an importer cannot use a re-exported function with a stripped or weakened effect row.

**Positive tests:**

- `crates/corvid-types/src/tests.rs::python_import_with_unsafe_effect_warns`

**Adversarial tests:**

- `crates/corvid-types/src/tests.rs::python_import_without_effects_is_rejected`

### Grounded provenance

#### `grounded.provenance_required`
- **class**: static
- **phase**: typecheck

Constructing a `Grounded<T>` value requires citing a source; unsourced `Grounded` construction is a compile error.

**Positive tests:**

- `crates/corvid-types/src/tests.rs::mutation_direct_grounded_return_with_retrieval_chain_is_ok`

**Adversarial tests:**

- `crates/corvid-types/src/tests.rs::mutation_grounded_return_without_retrieval_errors`

#### `grounded.propagation_across_calls`
- **class**: static
- **phase**: typecheck

Provenance is preserved across function boundaries — a `Grounded<T>` returned from a callee retains its citation chain into the caller without separate annotation.

**Positive tests:**

- `crates/corvid-types/src/tests.rs::mutation_intermediate_local_preserves_grounded_provenance`
- `crates/corvid-types/src/tests.rs::mutation_cross_agent_grounded_provenance_flows`

**Adversarial tests:**

- `crates/corvid-types/src/tests.rs::mutation_ungrounded_prompt_inputs_do_not_create_grounded_output`

### Budgets

#### `budget.compile_time_ceiling`
- **class**: static
- **phase**: typecheck

An agent annotated `@budget($X)` fails compile if the sum of statically known per-call costs along any reachable path exceeds `$X`.

**Positive tests:**

- `crates/corvid-types/src/tests.rs::multi_dimensional_budget_within_bound_is_clean`
- `crates/corvid-types/src/tests.rs::mutation_budget_within_limit_is_ok`

**Adversarial tests:**

- `crates/corvid-types/src/tests.rs::multi_dimensional_budget_violation_reports_path`
- `crates/corvid-types/src/tests.rs::mutation_budget_exceeded_is_effect_violation`

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

**Positive tests:**

- `crates/corvid-types/src/tests.rs::min_confidence_passes_when_composed_confidence_meets_floor`
- `crates/corvid-types/src/tests.rs::tagged_invalid_confidence_carries_confidence_guarantee_id`

**Adversarial tests:**

- `crates/corvid-types/src/tests.rs::min_confidence_fires_when_composed_confidence_below_floor`
- `crates/corvid-types/src/tests.rs::effect_confidence_out_of_range_is_rejected`

### Replay determinism

#### `replay.deterministic_pure_path`
- **class**: runtime_checked
- **phase**: runtime

A trace recorded from a `@replayable` agent reproduces deterministically across `corvid replay` invocations on the same compiled binary; non-deterministic divergence raises the documented replay-divergence error.

**Positive tests:**

- `crates/corvid-types/src/tests.rs::replayable_agent_with_pure_body_compiles_clean`
- `crates/corvid-types/src/tests.rs::deterministic_agent_with_pure_body_compiles_clean`

**Adversarial tests:**

- `crates/corvid-types/src/tests.rs::deterministic_agent_calling_tool_is_rejected`
- `crates/corvid-types/src/tests.rs::deterministic_agent_calling_prompt_is_rejected`

#### `replay.trace_signature`
- **class**: runtime_checked
- **phase**: runtime

Trace receipts produced with `--sign` carry a DSSE envelope whose signature `corvid receipt verify` checks against the supplied verifying key.

**Positive tests:**

- `crates/corvid-cli/tests/receipt_signing.rs::sign_then_verify_roundtrips_end_to_end`

**Adversarial tests:**

- `crates/corvid-cli/tests/receipt_signing.rs::verify_rejects_envelope_signed_with_different_key`
- `crates/corvid-cli/tests/receipt_signing.rs::verify_rejects_tampered_payload`

### Provenance traces

#### `provenance_trace.receipt_signature`
- **class**: runtime_checked
- **phase**: runtime

`corvid receipt verify` rejects any DSSE-wrapped receipt whose signature does not validate against the supplied verifying key, with a non-zero exit and the documented `verification failed` message.

**Positive tests:**

- `crates/corvid-cli/tests/receipt_signing.rs::sign_then_verify_roundtrips_end_to_end`

**Adversarial tests:**

- `crates/corvid-cli/tests/receipt_signing.rs::verify_rejects_envelope_signed_with_different_key`
- `crates/corvid-cli/tests/receipt_signing.rs::verify_rejects_tampered_payload`

### ABI descriptor

#### `abi_descriptor.cdylib_emission`
- **class**: static
- **phase**: codegen

Every `corvid build --target=cdylib` output exports a `CORVID_ABI_DESCRIPTOR` symbol whose payload is the canonical effect/approval/provenance surface for the compiled program.

**Positive tests:**

- `crates/corvid-codegen-cl/tests/cdylib_emission.rs::cdylib_target_produces_shared_library_file`
- `crates/corvid-codegen-cl/tests/cdylib_emission.rs::cdylib_symbol_is_resolvable_via_dlopen`

**Adversarial tests:**

- `crates/corvid-cli/tests/build_cdylib.rs::cli_build_cdylib_fails_cleanly_on_non_scalar_signature`

#### `abi_descriptor.byte_determinism`
- **class**: static
- **phase**: codegen

Two byte-identical Corvid sources compiled with the same toolchain version produce byte-identical descriptor JSON; the descriptor is canonical, not pretty-printed.

**Positive tests:**

- `crates/corvid-abi/tests/determinism.rs::identical_source_produces_byte_identical_descriptor_modulo_generated_at`
- `crates/corvid-abi/tests/byte_fuzz_corpus.rs::descriptor_bytes_are_byte_identical_across_two_emissions_of_same_source`

**Adversarial tests:**

- `crates/corvid-abi/tests/byte_fuzz_corpus.rs::descriptor_section_rejects_random_byte_flips`

#### `abi_descriptor.bilateral_source_match`
- **class**: runtime_checked
- **phase**: abi_verify

`corvid-abi-verify --source <file> <cdylib>` independently rebuilds the ABI descriptor from source and byte-compares it against the embedded `CORVID_ABI_DESCRIPTOR` symbol; mismatch is rejected before host acceptance.

**Positive tests:**

- `crates/corvid-abi-verify/src/lib.rs::verifier_accepts_matching_cdylib_descriptor`
- `crates/corvid-abi-verify/src/lib.rs::verifier_accepts_matching_cdylib_with_imported_agent`

**Adversarial tests:**

- `crates/corvid-abi-verify/src/lib.rs::verifier_rejects_source_descriptor_mismatch`

### ABI attestation

#### `abi_attestation.envelope_signature`
- **class**: runtime_checked
- **phase**: abi_verify

`corvid receipt verify-abi` rejects a signed cdylib whose attestation envelope does not validate against the supplied verifying key, exiting 1 with `attestation verification failed`.

**Positive tests:**

- `crates/corvid-cli/tests/abi_attestation.rs::signed_cdylib_verifies_against_matching_key`
- `crates/corvid-abi/tests/byte_fuzz_corpus.rs::signing_key_round_trip_baseline`

**Adversarial tests:**

- `crates/corvid-cli/tests/abi_attestation.rs::signed_cdylib_rejects_wrong_key`
- `crates/corvid-abi/tests/byte_fuzz_corpus.rs::dsse_envelope_signature_tampering_is_rejected`
- `crates/corvid-abi/tests/byte_fuzz_corpus.rs::dsse_envelope_payload_tampering_is_rejected`
- `crates/corvid-abi/tests/byte_fuzz_corpus.rs::dsse_envelope_payload_type_swap_is_rejected`
- `crates/corvid-abi/tests/byte_fuzz_corpus.rs::attestation_section_rejects_every_magic_or_version_byte_flip`
- `crates/corvid-abi/tests/byte_fuzz_corpus.rs::attestation_section_body_mutations_break_signature_verification`

#### `abi_attestation.descriptor_match`
- **class**: runtime_checked
- **phase**: abi_verify

After signature validation, the recovered attestation payload must bit-match the embedded `CORVID_ABI_DESCRIPTOR`; mismatch is rejected even when the signature is valid.

**Positive tests:**

- `crates/corvid-cli/tests/abi_attestation.rs::signed_cdylib_verifies_against_matching_key`

**Adversarial tests:**

- `crates/corvid-cli/tests/abi_attestation.rs::signed_cdylib_rejects_wrong_key`

#### `abi_attestation.absent_reports_unsigned`
- **class**: runtime_checked
- **phase**: abi_verify

`corvid receipt verify-abi` on a cdylib lacking the `CORVID_ABI_ATTESTATION` symbol exits 2 with the documented `unsigned` message, leaving the host policy to decide whether to accept it.

**Positive tests:**

- `crates/corvid-cli/tests/abi_attestation.rs::signed_cdylib_verifies_against_matching_key`

**Adversarial tests:**

- `crates/corvid-cli/tests/abi_attestation.rs::unsigned_cdylib_reports_absent_attestation`

#### `abi_attestation.sign_requires_claim_coverage`
- **class**: static
- **phase**: codegen

`corvid build --target=cdylib --sign` refuses to sign when any contract declared by the source lacks a non-out-of-scope guarantee id in the descriptor's signed claim set.

**Positive tests:**

- `crates/corvid-driver/src/build.rs::signed_claim_coverage_accepts_registered_contracts`

**Adversarial tests:**

- `crates/corvid-driver/src/build.rs::signed_claim_coverage_rejects_missing_declared_contract_id`
- `crates/corvid-driver/src/build.rs::signed_claim_coverage_rejects_out_of_scope_contract_id`

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
