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
| `approval.reachable_entrypoints_require_contract` | approval | static | typecheck |
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
| `jobs.cron_schedule_durable` | jobs | runtime_checked | runtime |
| `jobs.retry_budget_bound` | jobs | out_of_scope | runtime |
| `jobs.idempotency_key_uniqueness` | jobs | runtime_checked | runtime |
| `jobs.lease_exclusivity` | jobs | runtime_checked | runtime |
| `jobs.durable_resume` | jobs | runtime_checked | runtime |
| `jobs.cron_dst_correct` | jobs | runtime_checked | runtime |
| `jobs.approval_wait_resume` | jobs | out_of_scope | runtime |
| `jobs.loop_bounds_enforced` | jobs | out_of_scope | runtime |
| `auth.session_rotation_on_privilege_change` | auth | out_of_scope | runtime |
| `auth.api_key_at_rest_hashed` | auth | runtime_checked | runtime |
| `auth.jwt_kid_rotation` | auth | runtime_checked | runtime |
| `auth.oauth_pkce_required` | auth | runtime_checked | runtime |
| `auth.csrf_double_submit` | auth | out_of_scope | runtime |
| `tenant.cross_tenant_compile_error` | auth | out_of_scope | typecheck |
| `approval.policy_clause_static_check` | auth | out_of_scope | typecheck |
| `approval.batch_equivalence_typed` | auth | out_of_scope | typecheck |
| `approval.confused_deputy_typecheck` | auth | out_of_scope | typecheck |
| `connector.scope_minimum_enforced` | connector | out_of_scope | runtime |
| `connector.write_requires_approval` | connector | out_of_scope | typecheck |
| `connector.rate_limit_respects_provider` | connector | out_of_scope | runtime |
| `connector.contract_drift_detected` | connector | out_of_scope | runtime |
| `connector.webhook_signature_verified` | connector | out_of_scope | runtime |
| `connector.replay_quarantine` | connector | out_of_scope | runtime |
| `observability.otel_conformance` | observability | runtime_checked | runtime |
| `observability.lineage_completeness` | observability | runtime_checked | runtime |
| `platform.host_kernel_compromise` | platform | out_of_scope | platform |
| `platform.signing_key_compromise` | platform | out_of_scope | platform |
| `platform.toolchain_compromise` | platform | out_of_scope | platform |
| `platform.hosted_registry_available` | platform | out_of_scope | platform |

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

#### `approval.reachable_entrypoints_require_contract`
- **class**: static
- **phase**: typecheck

Externally reachable routes, schedules, and exported agents are walked through their reachable agent calls; any reachable `@dangerous` tool call must still have a matching lexical approval contract.

**Positive tests:**

- `crates/corvid-types/src/tests.rs::server_route_approve_authorizes_dangerous_tool`

**Adversarial tests:**

- `crates/corvid-types/src/tests.rs::server_route_reachability_reports_helper_without_approval`
- `crates/corvid-types/src/tests.rs::schedule_reachability_reports_job_without_approval`

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

### Durable jobs

#### `jobs.cron_schedule_durable`
- **class**: runtime_checked
- **phase**: runtime

A `schedule "cron" zone "…" -> job(args)` declaration persists to the durable queue store and survives process restart. Slice 35-N walks `Decl::Schedule` so a signed cdylib that declares a cron schedule cannot ship without this guarantee in its descriptor.

**Positive tests:**

- `crates/corvid-driver/src/build.rs::signed_claim_coverage_walks_schedule_decl`

**Adversarial tests:**

- `crates/corvid-driver/src/build.rs::signed_claim_coverage_rejects_schedule_without_jobs_coverage`

#### `jobs.retry_budget_bound`
- **class**: out_of_scope
- **phase**: runtime

`@retry(max_attempts: N, backoff: ...)` bounds the runtime retry envelope of a job so a transient failure cannot escalate into unbounded re-spend.

> **Why out of scope:** The runtime queue and lease envelopes are shipped, but `@retry` is not yet a parser-level attribute. Slice 38K promotes this row to `RuntimeChecked` when the multi-worker runner consumes the attribute end-to-end.

#### `jobs.idempotency_key_uniqueness`
- **class**: runtime_checked
- **phase**: runtime

Across N concurrent workers, exactly one durable queue row exists for a given non-null idempotency key. Enforced by a partial UNIQUE INDEX on `queue_jobs(idempotency_key) WHERE idempotency_key IS NOT NULL` in the SQLite schema, plus the existing `enqueue_typed_idempotent` collision-fallback path that returns the surviving row when the insert hits the UNIQUE constraint.

**Positive tests:**

- `crates/corvid-runtime/src/queue.rs::durable_queue_idempotency_key_collapses_duplicate_jobs`

**Adversarial tests:**

- `crates/corvid-runtime/tests/durability_corpus.rs::t38l_d1_four_workers_collapse_to_one_row`

#### `jobs.lease_exclusivity`
- **class**: runtime_checked
- **phase**: runtime

A job lease prevents two workers from running the same job concurrently. Slice 38K's `WorkerPool` over `DurableQueueRuntime` runs N tokio tasks each contesting `lease_next_at`; the SQLite UPDATE that flips `pending` → `leased` is atomic, so exactly one worker wins each contention round. Lease expiry plus a fresh worker re-leasing is shipped (slice 38L's D3 test); heartbeat extension for long-running steps remains a follow-up.

**Positive tests:**

- `crates/corvid-runtime/src/worker_pool.rs::t38k_pool_runs_each_job_exactly_once`

**Adversarial tests:**

- `crates/corvid-runtime/src/worker_pool.rs::t38k_two_workers_cannot_both_lease_same_job`
- `crates/corvid-runtime/src/worker_pool.rs::t38k_pool_drains_gracefully_without_claiming_new_work`

#### `jobs.durable_resume`
- **class**: runtime_checked
- **phase**: runtime

A worker that drops uncleanly mid-step (the SIGKILL surrogate the queue runtime is responsible for) leaves behind durable checkpoint rows; a fresh worker that opens the same SQLite file after the lease TTL elapses can re-lease the job and resume from those checkpoints. SQLite WAL fsync makes this property structural. The count-bounded `no double LLM call` extension joins the Phase 21 Replay corpus when step-skip semantics land at the VM layer.

**Positive tests:**

- `crates/corvid-runtime/src/queue.rs::durable_queue_records_ordered_agent_checkpoints`

**Adversarial tests:**

- `crates/corvid-runtime/tests/durability_corpus.rs::t38l_d3_checkpoints_survive_unclean_shutdown`

#### `jobs.cron_dst_correct`
- **class**: runtime_checked
- **phase**: runtime

Cron schedules expressed in `America/New_York` (and other DST-observing timezones) produce monotonic UTC fire times across the spring-forward and fall-back transitions, with no duplicates and no fire at the non-existent local instant. `chrono-tz` is wired into the queue runtime; the cron-crate's `Schedule::after` iterator is timezone-aware.

**Positive tests:**

- `crates/corvid-runtime/tests/durability_corpus.rs::t38l_d2_dst_spring_forward_is_deterministic`
- `crates/corvid-runtime/tests/durability_corpus.rs::t38l_d2_dst_fall_back_is_monotonic`

**Adversarial tests:**

- `crates/corvid-runtime/tests/durability_corpus.rs::t38l_d2_dst_spring_forward_is_deterministic`

#### `jobs.approval_wait_resume`
- **class**: out_of_scope
- **phase**: runtime

`await_approval` pauses a job until an approval token arrives, expires, or is denied; the resume path writes the audit transition.

> **Why out of scope:** Runtime approval-wait state ships; `await_approval` is not yet a parser-level keyword. Slice 38K (or a follow-up syntax slice) promotes.

#### `jobs.loop_bounds_enforced`
- **class**: out_of_scope
- **phase**: runtime

Agent loops driven by jobs honor max-steps, max-wall-time, max-spend, and max-tool-calls; exceeding any bound escalates or terminates with trace evidence.

> **Why out of scope:** Loop-bound envelopes ship; the multi-worker runner that enforces them across crash + restart is not yet wired. Slice 38K promotes.

### Auth and approvals

#### `auth.session_rotation_on_privilege_change`
- **class**: out_of_scope
- **phase**: runtime

A session id rotates on privilege escalation (role upgrade, password change) so a stolen pre-escalation cookie cannot exercise the post-escalation privilege.

> **Why out of scope:** Session table ships; rotation hook is not yet wired through a parser-level `auth` block. Slice 39L promotes.

#### `auth.api_key_at_rest_hashed`
- **class**: runtime_checked
- **phase**: runtime

API keys are stored only as Argon2id hashes; the plaintext leaves Corvid memory exactly once at issuance and is never logged. Verified by the existing `hash_api_key_secret`/`verify_api_key_secret` path in `corvid-runtime/src/auth.rs`.

**Positive tests:**

- `crates/corvid-runtime/src/auth.rs::api_key_runtime_resolves_service_actor_with_argon2_hash_and_redacted_audit`

**Adversarial tests:**

- `crates/corvid-runtime/src/auth.rs::api_key_runtime_rejects_wrong_tenant_revoked_expired_and_user_actors`

#### `auth.jwt_kid_rotation`
- **class**: runtime_checked
- **phase**: runtime

JWT verification fetches the JWKS, picks the key by `kid`, verifies the signature with `jsonwebtoken`, and rejects tokens whose `kid` is missing from the current JWKS, whose alg does not match the contract, whose signature fails to verify, whose exp/iss/aud do not align with the contract, or whose required subject/tenant claim is missing. Out-of-scope at Phase 39 base; promoted to `RuntimeChecked` by slice 39K when `corvid-runtime/src/jwt_verify.rs` shipped.

**Positive tests:**

- `crates/corvid-runtime/src/jwt_verify.rs::parse_alg_accepts_supported_and_refuses_others`
- `crates/corvid-runtime/src/jwt_verify.rs::decoding_key_for_rsa_jwk_constructs`
- `crates/corvid-runtime/src/jwt_verify.rs::error_slugs_are_stable_for_audit_log`

**Adversarial tests:**

- `crates/corvid-runtime/src/jwt_verify.rs::kid_downgrade_returns_kid_not_found`
- `crates/corvid-runtime/src/jwt_verify.rs::header_alg_must_match_contract_alg`
- `crates/corvid-runtime/src/jwt_verify.rs::alg_none_in_header_is_refused`
- `crates/corvid-runtime/src/jwt_verify.rs::malformed_token_is_refused_before_fetch`
- `crates/corvid-runtime/src/jwt_verify.rs::jwks_fetch_failure_is_surfaced`
- `crates/corvid-runtime/src/jwt_verify.rs::decoding_key_for_rejects_rsa_without_n`
- `crates/corvid-runtime/src/jwt_verify.rs::decoding_key_for_rejects_unknown_kty`

#### `auth.oauth_pkce_required`
- **class**: runtime_checked
- **phase**: runtime

OAuth callback state requires PKCE for public clients; the state record carries the code-verifier hash and is single-use, tenant-scoped, and expiry-bound.

**Positive tests:**

- `crates/corvid-runtime/src/auth.rs::oauth_callback_state_is_hashed_single_use_and_restart_safe`

**Adversarial tests:**

- `crates/corvid-runtime/src/auth.rs::oauth_callback_rejects_expired_and_cross_tenant_state`

#### `auth.csrf_double_submit`
- **class**: out_of_scope
- **phase**: runtime

CSRF protection on cookie-bearing requests uses a double-submit token verified by HMAC-SHA256.

> **Why out of scope:** Token shape is documented in the design brief; the middleware path that enforces it on every cookie-bearing POST/PUT/PATCH/DELETE is not yet wired into the generated axum server. Slice 39L promotes.

#### `tenant.cross_tenant_compile_error`
- **class**: out_of_scope
- **phase**: typecheck

A function whose actor came from tenant A may not pass a record owned by tenant B to a tool that writes back into A — the typechecker rejects the cross-tenant reference.

> **Why out of scope:** Tenant tagging exists in runtime envelopes but the parser-level `tenant Org { ... }` block does not exist yet. Slice 39L (parser surface) plus a typecheck slice promotes this row to `Static`.

#### `approval.policy_clause_static_check`
- **class**: out_of_scope
- **phase**: typecheck

An `approval Name:` block's `policy { ... }` clause type-checks at compile time so a malformed predicate (wrong field name, wrong type, undeclared role) cannot ship.

> **Why out of scope:** Approval store ships; the `approval Name:` parser-level block does not. Slice 39L promotes.

#### `approval.batch_equivalence_typed`
- **class**: out_of_scope
- **phase**: typecheck

An `approval ... batch_with: same_tool, same_data_class, same_role` clause groups equivalent approvals so a reviewer can approve one record and have N equivalent-by-typed-shape records auto-resolve.

> **Why out of scope:** Batch logic exists in the approval queue runtime but the `batch_with` clause has no parser surface. Slice 39L promotes.

#### `approval.confused_deputy_typecheck`
- **class**: out_of_scope
- **phase**: typecheck

A reachable path from any route or job to a `@dangerous` tool must have an `approve` token whose `required_role` covers every reachable caller — otherwise typecheck rejects.

> **Why out of scope:** Lexical-scope approval check ships (`approval.token_lexical_only`); the cross-call reachability extension into routes/jobs is not yet wired. Slice 39L promotes.

### Connectors

#### `connector.scope_minimum_enforced`
- **class**: out_of_scope
- **phase**: runtime

A connector cannot use a scope its manifest does not declare; the runtime rejects requests whose required scope is not in the declared scope set.

> **Why out of scope:** Manifest parser ships; the runtime real-mode call path (which is the only place where scope is consulted against a live token) returns `RealModeNotBound`. Slice 41K promotes.

#### `connector.write_requires_approval`
- **class**: out_of_scope
- **phase**: typecheck

A connector method whose effect set names a write (`gmail.send`, `slack.post`, `github.create_issue`) reaches typecheck only when its caller has a matching `approve` boundary in lexical scope.

> **Why out of scope:** Manifest declares write effects but the connector AST surface is not yet parser-level — connectors today are configured by Rust data, not source. Slice 41L promotes.

#### `connector.rate_limit_respects_provider`
- **class**: out_of_scope
- **phase**: runtime

A connector honors the provider's rate-limit advice (`Retry-After`, 429, 5xx) using the limit declared in the manifest as an upper bound.

> **Why out of scope:** Rate-limit envelope exists; the real-mode HTTP retry path that consumes it is not implemented. Slice 41K promotes.

#### `connector.contract_drift_detected`
- **class**: out_of_scope
- **phase**: runtime

`corvid connectors check --live` compares the manifest to the live (or recorded-cassette) provider response shape and exits non-zero when fields drift.

> **Why out of scope:** `corvid connectors check` CLI is unwired. Slice 41L promotes.

#### `connector.webhook_signature_verified`
- **class**: out_of_scope
- **phase**: runtime

Inbound webhook payloads from Slack, GitHub, and Linear are HMAC-SHA256 verified against the manifest's `webhook_signed_by` secret reference; failure rejects the payload before any handler runs.

> **Why out of scope:** `hmac` and `sha2` are not imported by `corvid-connector-runtime`. Slice 41M promotes.

#### `connector.replay_quarantine`
- **class**: out_of_scope
- **phase**: runtime

A connector running in replay mode must not issue real provider calls; the runtime quarantines outbound HTTP when the active mode is `Replay`.

> **Why out of scope:** Replay mode exists in the connector runtime but real mode is `RealModeNotBound`, so the quarantine guard is not exercisable end-to-end. Slice 41K promotes.

### Observability and evals

#### `observability.otel_conformance`
- **class**: runtime_checked
- **phase**: runtime

Lineage events flow through the standard `opentelemetry` + `opentelemetry-otlp` SDK and emit OTLP/HTTP spans whose attributes carry `corvid.guarantee_id`, `corvid.cost_usd`, `corvid.approval_id`, `corvid.replay_key`. The attribute set is constructed by `corvid_runtime::otel_sdk_export::corvid_span_attributes` and the live wire path is exercised by the docker-compose Jaeger harness in `docs/observability-conformance.md`.

**Positive tests:**

- `crates/corvid-runtime/src/otel_sdk_export.rs::span_attributes_include_corvid_named_keys`
- `crates/corvid-runtime/src/otel_sdk_export.rs::span_name_uses_corvid_prefix_with_kind`
- `crates/corvid-runtime/src/otel_sdk_export.rs::span_kind_maps_lineage_to_otel`

**Adversarial tests:**

- `crates/corvid-runtime/src/otel_sdk_export.rs::span_attributes_omit_missing_optional_keys`
- `crates/corvid-runtime/src/otel_sdk_export.rs::sdk_exporter_reaches_in_process_otlp_receiver`

#### `observability.lineage_completeness`
- **class**: runtime_checked
- **phase**: runtime

Every lineage event carries a (trace_id, span_id) pair plus parent linkage when a parent exists, so a SQL JOIN against the local trace store reconstructs the route → job → agent → prompt → tool → approval → DB tree. Validated on every event via `corvid_runtime::lineage::validate_lineage`.

**Positive tests:**

- `crates/corvid-runtime/src/lineage.rs::lineage_ids_are_stable_and_parented_across_backend_kinds`

**Adversarial tests:**

- `crates/corvid-runtime/src/lineage.rs::lineage_validation_fails_closed_for_missing_parent_or_duplicate_root`

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

#### `platform.hosted_registry_available`
- **class**: out_of_scope
- **phase**: platform

A Corvid-operated public package registry service that serves the published index format and source artifacts.

> **Why out of scope:** No hosted Corvid-operated registry service runs at v1.0. The CLI ships the published index format + signed-publish tooling (`corvid package publish`, `verify-registry`, `verify-lock`) and accepts any user-supplied `--url-base` (file://, self-hosted https, S3, CDN). A hosted public registry is post-v1.0 work; see `docs/package-manager-scope.md` for the full boundary.

## Updating this document

This file is generated. To change a description, add a new guarantee, or move an entry between `static` /
`runtime_checked` / `out_of_scope`, edit `crates/corvid-guarantees/src/lib.rs` and run:

```
cargo run -q -p corvid-cli -- contract regen-doc docs/core-semantics.md
```

Then commit the regenerated file together with the registry change. CI fails if the committed text drifts from the registry — there is no quiet way to evolve the spec away from the implementation.
