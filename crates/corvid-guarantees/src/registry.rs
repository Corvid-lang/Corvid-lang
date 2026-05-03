//! Canonical guarantee data + lookup helpers — slice 35
//! / contract registry, decomposed in Phase 20j-A8.
//!
//! `GUARANTEE_REGISTRY` is the single source of truth for every
//! public Corvid guarantee. The lookup helpers here all walk the
//! registry slice; nothing else is allowed to query it directly,
//! so this file owns "what's in the registry and how to read it."
//!
//! Honesty rules over the registry data live in [`super::validate`]
//! and are enforced by [`super::validate::validate_slice`]. Doc
//! generation lives in [`super::render`]. The signed-cdylib claim
//! whitelist lives in [`super::signed_claim`].

use super::types::{Guarantee, GuaranteeClass, GuaranteeKind, Phase};

/// Canonical guarantee table.
///
/// Order matters only for stable doc generation — the generator
/// (Slice 35-D) emits rows in declaration order, so adding a new
/// guarantee at the bottom keeps the existing doc stable. Entries
/// that conceptually belong together are grouped by kind.
pub static GUARANTEE_REGISTRY: &[Guarantee] = &[
    // ----- Approval boundaries ------------------------------------
    Guarantee {
        id: "approval.dangerous_call_requires_token",
        kind: GuaranteeKind::Approval,
        class: GuaranteeClass::Static,
        phase: Phase::TypeCheck,
        description:
            "A call site invoking a `@dangerous` tool must have an `approve` \
             token lexically in scope; otherwise the typechecker rejects \
             the program.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-types/src/tests.rs::dangerous_tool_with_matching_approve_is_ok",
        ],
        adversarial_test_refs: &[
            "crates/corvid-types/src/tests.rs::dangerous_tool_without_approve_is_compile_error",
            "crates/corvid-types/src/tests.rs::tagged_unapproved_dangerous_call_carries_approval_guarantee_id",
        ],
    },
    Guarantee {
        id: "approval.token_lexical_only",
        kind: GuaranteeKind::Approval,
        class: GuaranteeClass::Static,
        phase: Phase::TypeCheck,
        description:
            "Approval tokens are lexically scoped — they cannot be returned, \
             stored in fields, or passed across opaque boundaries to \
             unlock a call site outside the original `approve` block.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-types/src/tests.rs::outer_approve_authorizes_inner_call",
        ],
        adversarial_test_refs: &[
            "crates/corvid-types/src/tests.rs::approve_does_not_leak_out_of_if_branch",
            "crates/corvid-types/src/tests.rs::mutation_nested_inner_approve_does_not_authorize_outer_call",
        ],
    },
    Guarantee {
        id: "approval.dangerous_marker_preserved",
        kind: GuaranteeKind::Approval,
        class: GuaranteeClass::Static,
        phase: Phase::TypeCheck,
        description:
            "A `@dangerous` marker cannot be erased by re-exporting or \
             aliasing the tool through another module — every public \
             alias preserves the original danger annotation.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-types/tests/source_bypass_corpus.rs::baseline_for_alias_compiles_clean",
        ],
        adversarial_test_refs: &[
            "crates/corvid-types/src/tests.rs::adversarial_source_mutator_import_use_alias_dangerous_tool_is_tagged",
            "crates/corvid-types/tests/source_bypass_corpus.rs::mutator_drops_approve_through_mock_alias_triggers_token_guarantee",
        ],
    },
    Guarantee {
        id: "approval.reachable_entrypoints_require_contract",
        kind: GuaranteeKind::Approval,
        class: GuaranteeClass::Static,
        phase: Phase::TypeCheck,
        description:
            "Externally reachable routes, schedules, and exported agents \
             are walked through their reachable agent calls; any reachable \
             `@dangerous` tool call must still have a matching lexical \
             approval contract.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-types/src/tests.rs::server_route_approve_authorizes_dangerous_tool",
        ],
        adversarial_test_refs: &[
            "crates/corvid-types/src/tests.rs::server_route_reachability_reports_helper_without_approval",
            "crates/corvid-types/src/tests.rs::schedule_reachability_reports_job_without_approval",
        ],
    },
    // ----- Effect rows --------------------------------------------
    Guarantee {
        id: "effect_row.body_completeness",
        kind: GuaranteeKind::EffectRow,
        class: GuaranteeClass::Static,
        phase: Phase::TypeCheck,
        description:
            "A function's declared effect row must cover every effect \
             actually produced by its body (including effects of called \
             functions); under-reporting is a compile error.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-types/src/tests.rs::mutation_tool_uses_declared_effect_is_ok",
        ],
        adversarial_test_refs: &[
            "crates/corvid-types/src/tests.rs::mutation_baseline_trust_violation_exists",
            "crates/corvid-types/src/tests.rs::mutation_multiple_effects_on_one_tool_compose_cost_and_trust",
        ],
    },
    Guarantee {
        id: "effect_row.caller_propagation",
        kind: GuaranteeKind::EffectRow,
        class: GuaranteeClass::Static,
        phase: Phase::TypeCheck,
        description:
            "Callers inherit the union of their callees' effects unless \
             they declare a wider row; callers cannot silently shrink the \
             effect surface.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-types/src/tests.rs::sub_agent_costs_propagate_into_outer_agent",
        ],
        adversarial_test_refs: &[
            "crates/corvid-types/src/tests.rs::mutation_inner_agent_effects_propagate_to_outer_agent",
        ],
    },
    Guarantee {
        id: "effect_row.import_boundary",
        kind: GuaranteeKind::EffectRow,
        class: GuaranteeClass::Static,
        phase: Phase::Resolve,
        description:
            "Cross-module imports preserve effect annotations exactly; \
             an importer cannot use a re-exported function with a \
             stripped or weakened effect row.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-types/src/tests.rs::python_import_with_unsafe_effect_warns",
        ],
        adversarial_test_refs: &[
            "crates/corvid-types/src/tests.rs::python_import_without_effects_is_rejected",
        ],
    },
    // ----- Grounded<T> --------------------------------------------
    Guarantee {
        id: "grounded.provenance_required",
        kind: GuaranteeKind::Grounded,
        class: GuaranteeClass::Static,
        phase: Phase::TypeCheck,
        description:
            "Constructing a `Grounded<T>` value requires citing a source; \
             unsourced `Grounded` construction is a compile error.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-types/src/tests.rs::mutation_direct_grounded_return_with_retrieval_chain_is_ok",
        ],
        adversarial_test_refs: &[
            "crates/corvid-types/src/tests.rs::mutation_grounded_return_without_retrieval_errors",
        ],
    },
    Guarantee {
        id: "grounded.propagation_across_calls",
        kind: GuaranteeKind::Grounded,
        class: GuaranteeClass::Static,
        phase: Phase::TypeCheck,
        description:
            "Provenance is preserved across function boundaries — a \
             `Grounded<T>` returned from a callee retains its citation \
             chain into the caller without separate annotation.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-types/src/tests.rs::mutation_intermediate_local_preserves_grounded_provenance",
            "crates/corvid-types/src/tests.rs::mutation_cross_agent_grounded_provenance_flows",
        ],
        adversarial_test_refs: &[
            "crates/corvid-types/src/tests.rs::mutation_ungrounded_prompt_inputs_do_not_create_grounded_output",
        ],
    },
    // ----- Budgets ------------------------------------------------
    Guarantee {
        id: "budget.compile_time_ceiling",
        kind: GuaranteeKind::Budget,
        class: GuaranteeClass::Static,
        phase: Phase::TypeCheck,
        description:
            "An agent annotated `@budget($X)` fails compile if the sum of \
             statically known per-call costs along any reachable path \
             exceeds `$X`.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-types/src/tests.rs::multi_dimensional_budget_within_bound_is_clean",
            "crates/corvid-types/src/tests.rs::mutation_budget_within_limit_is_ok",
        ],
        adversarial_test_refs: &[
            "crates/corvid-types/src/tests.rs::multi_dimensional_budget_violation_reports_path",
            "crates/corvid-types/src/tests.rs::mutation_budget_exceeded_is_effect_violation",
        ],
    },
    Guarantee {
        id: "budget.runtime_termination",
        kind: GuaranteeKind::Budget,
        class: GuaranteeClass::OutOfScope,
        phase: Phase::Runtime,
        description:
            "Live runtime termination of an agent when actual runtime cost \
             crosses the `@budget($X)` threshold mid-execution.",
        out_of_scope_reason:
            "Today Corvid enforces budgets at compile time via \
             `budget.compile_time_ceiling`, and the runtime observes \
             per-call cost in trace events; live mid-execution \
             termination on threshold crossing is not yet implemented. \
             A future slice can promote this entry back to \
             `RuntimeChecked` once the enforcement ships. The compile-time \
             ceiling is the load-bearing guarantee for v1.0.",
        positive_test_refs: &[],
        adversarial_test_refs: &[],
    },
    // ----- Confidence ---------------------------------------------
    Guarantee {
        id: "confidence.min_threshold",
        kind: GuaranteeKind::Confidence,
        class: GuaranteeClass::Static,
        phase: Phase::TypeCheck,
        description:
            "An agent annotated `@min_confidence(X)` requires every input \
             carrying a confidence tag to meet `X`; lower-confidence \
             inputs are rejected at the call site.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-types/src/tests.rs::min_confidence_passes_when_composed_confidence_meets_floor",
            "crates/corvid-types/src/tests.rs::tagged_invalid_confidence_carries_confidence_guarantee_id",
        ],
        adversarial_test_refs: &[
            "crates/corvid-types/src/tests.rs::min_confidence_fires_when_composed_confidence_below_floor",
            "crates/corvid-types/src/tests.rs::effect_confidence_out_of_range_is_rejected",
        ],
    },
    // ----- Replay -------------------------------------------------
    Guarantee {
        id: "replay.deterministic_pure_path",
        kind: GuaranteeKind::Replay,
        class: GuaranteeClass::RuntimeChecked,
        phase: Phase::Runtime,
        description:
            "A trace recorded from a `@replayable` agent reproduces \
             deterministically across `corvid replay` invocations on the \
             same compiled binary; non-deterministic divergence raises \
             the documented replay-divergence error.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-types/src/tests.rs::replayable_agent_with_pure_body_compiles_clean",
            "crates/corvid-types/src/tests.rs::deterministic_agent_with_pure_body_compiles_clean",
        ],
        adversarial_test_refs: &[
            "crates/corvid-types/src/tests.rs::deterministic_agent_calling_tool_is_rejected",
            "crates/corvid-types/src/tests.rs::deterministic_agent_calling_prompt_is_rejected",
        ],
    },
    Guarantee {
        id: "replay.trace_signature",
        kind: GuaranteeKind::Replay,
        class: GuaranteeClass::RuntimeChecked,
        phase: Phase::Runtime,
        description:
            "Trace receipts produced with `--sign` carry a DSSE envelope \
             whose signature `corvid receipt verify` checks against the \
             supplied verifying key.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-cli/tests/receipt_signing.rs::sign_then_verify_roundtrips_end_to_end",
        ],
        adversarial_test_refs: &[
            "crates/corvid-cli/tests/receipt_signing.rs::verify_rejects_envelope_signed_with_different_key",
            "crates/corvid-cli/tests/receipt_signing.rs::verify_rejects_tampered_payload",
        ],
    },
    // ----- Provenance / receipts ----------------------------------
    Guarantee {
        id: "provenance_trace.receipt_signature",
        kind: GuaranteeKind::ProvenanceTrace,
        class: GuaranteeClass::RuntimeChecked,
        phase: Phase::Runtime,
        description:
            "`corvid receipt verify` rejects any DSSE-wrapped receipt \
             whose signature does not validate against the supplied \
             verifying key, with a non-zero exit and the documented \
             `verification failed` message.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-cli/tests/receipt_signing.rs::sign_then_verify_roundtrips_end_to_end",
        ],
        adversarial_test_refs: &[
            "crates/corvid-cli/tests/receipt_signing.rs::verify_rejects_envelope_signed_with_different_key",
            "crates/corvid-cli/tests/receipt_signing.rs::verify_rejects_tampered_payload",
        ],
    },
    // ----- ABI descriptor -----------------------------------------
    Guarantee {
        id: "abi_descriptor.cdylib_emission",
        kind: GuaranteeKind::AbiDescriptor,
        class: GuaranteeClass::Static,
        phase: Phase::Codegen,
        description:
            "Every `corvid build --target=cdylib` output exports a \
             `CORVID_ABI_DESCRIPTOR` symbol whose payload is the canonical \
             effect/approval/provenance surface for the compiled program.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-codegen-cl/tests/cdylib_emission.rs::cdylib_target_produces_shared_library_file",
            "crates/corvid-codegen-cl/tests/cdylib_emission.rs::cdylib_symbol_is_resolvable_via_dlopen",
        ],
        adversarial_test_refs: &[
            "crates/corvid-cli/tests/build_cdylib.rs::cli_build_cdylib_fails_cleanly_on_non_scalar_signature",
        ],
    },
    Guarantee {
        id: "abi_descriptor.byte_determinism",
        kind: GuaranteeKind::AbiDescriptor,
        class: GuaranteeClass::Static,
        phase: Phase::Codegen,
        description:
            "Two byte-identical Corvid sources compiled with the same \
             toolchain version produce byte-identical descriptor JSON; \
             the descriptor is canonical, not pretty-printed.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-abi/tests/determinism.rs::identical_source_produces_byte_identical_descriptor_modulo_generated_at",
            "crates/corvid-abi/tests/byte_fuzz_corpus.rs::descriptor_bytes_are_byte_identical_across_two_emissions_of_same_source",
        ],
        adversarial_test_refs: &[
            "crates/corvid-abi/tests/byte_fuzz_corpus.rs::descriptor_section_rejects_random_byte_flips",
        ],
    },
    Guarantee {
        id: "abi_descriptor.bilateral_source_match",
        kind: GuaranteeKind::AbiDescriptor,
        class: GuaranteeClass::RuntimeChecked,
        phase: Phase::AbiVerify,
        description:
            "`corvid-abi-verify --source <file> <cdylib>` independently \
             rebuilds the ABI descriptor from source and byte-compares it \
             against the embedded `CORVID_ABI_DESCRIPTOR` symbol; mismatch \
             is rejected before host acceptance.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-abi-verify/src/lib.rs::verifier_accepts_matching_cdylib_descriptor",
            "crates/corvid-abi-verify/src/lib.rs::verifier_accepts_matching_cdylib_with_imported_agent",
        ],
        adversarial_test_refs: &[
            "crates/corvid-abi-verify/src/lib.rs::verifier_rejects_source_descriptor_mismatch",
        ],
    },
    // ----- ABI attestation ----------------------------------------
    Guarantee {
        id: "abi_attestation.envelope_signature",
        kind: GuaranteeKind::AbiAttestation,
        class: GuaranteeClass::RuntimeChecked,
        phase: Phase::AbiVerify,
        description:
            "`corvid receipt verify-abi` rejects a signed cdylib whose \
             attestation envelope does not validate against the supplied \
             verifying key, exiting 1 with `attestation verification \
             failed`.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-cli/tests/abi_attestation.rs::signed_cdylib_verifies_against_matching_key",
            "crates/corvid-abi/tests/byte_fuzz_corpus.rs::signing_key_round_trip_baseline",
        ],
        adversarial_test_refs: &[
            "crates/corvid-cli/tests/abi_attestation.rs::signed_cdylib_rejects_wrong_key",
            "crates/corvid-abi/tests/byte_fuzz_corpus.rs::dsse_envelope_signature_tampering_is_rejected",
            "crates/corvid-abi/tests/byte_fuzz_corpus.rs::dsse_envelope_payload_tampering_is_rejected",
            "crates/corvid-abi/tests/byte_fuzz_corpus.rs::dsse_envelope_payload_type_swap_is_rejected",
            "crates/corvid-abi/tests/byte_fuzz_corpus.rs::attestation_section_rejects_every_magic_or_version_byte_flip",
            "crates/corvid-abi/tests/byte_fuzz_corpus.rs::attestation_section_body_mutations_break_signature_verification",
        ],
    },
    Guarantee {
        id: "abi_attestation.descriptor_match",
        kind: GuaranteeKind::AbiAttestation,
        class: GuaranteeClass::RuntimeChecked,
        phase: Phase::AbiVerify,
        description:
            "After signature validation, the recovered attestation \
             payload must bit-match the embedded \
             `CORVID_ABI_DESCRIPTOR`; mismatch is rejected even when \
             the signature is valid.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-cli/tests/abi_attestation.rs::signed_cdylib_verifies_against_matching_key",
        ],
        adversarial_test_refs: &[
            "crates/corvid-cli/tests/abi_attestation.rs::signed_cdylib_rejects_wrong_key",
        ],
    },
    Guarantee {
        id: "abi_attestation.absent_reports_unsigned",
        kind: GuaranteeKind::AbiAttestation,
        class: GuaranteeClass::RuntimeChecked,
        phase: Phase::AbiVerify,
        description:
            "`corvid receipt verify-abi` on a cdylib lacking the \
             `CORVID_ABI_ATTESTATION` symbol exits 2 with the documented \
             `unsigned` message, leaving the host policy to decide \
             whether to accept it.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-cli/tests/abi_attestation.rs::signed_cdylib_verifies_against_matching_key",
        ],
        adversarial_test_refs: &[
            "crates/corvid-cli/tests/abi_attestation.rs::unsigned_cdylib_reports_absent_attestation",
        ],
    },
    Guarantee {
        id: "abi_attestation.sign_requires_claim_coverage",
        kind: GuaranteeKind::AbiAttestation,
        class: GuaranteeClass::Static,
        phase: Phase::Codegen,
        description:
            "`corvid build --target=cdylib --sign` refuses to sign when \
             any contract declared by the source lacks a non-out-of-scope \
             guarantee id in the descriptor's signed claim set.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-driver/src/build/tests.rs::signed_claim_coverage_accepts_registered_contracts",
        ],
        adversarial_test_refs: &[
            "crates/corvid-driver/src/build/tests.rs::signed_claim_coverage_rejects_missing_declared_contract_id",
            "crates/corvid-driver/src/build/tests.rs::signed_claim_coverage_rejects_out_of_scope_contract_id",
        ],
    },
    // ----- Jobs (Phase 38) ---------------------------------------
    // These rows are placeholders so `validate_signed_claim_coverage`
    // can recognise the contract surfaces named by the developer-flow
    // doc when their parser-level keywords land. Each row gets
    // promoted to `Static` or `RuntimeChecked` by the audit-correction
    // slice that wires the surface end-to-end (38K/38L/38M).
    Guarantee {
        id: "jobs.cron_schedule_durable",
        kind: GuaranteeKind::Jobs,
        class: GuaranteeClass::RuntimeChecked,
        phase: Phase::Runtime,
        description:
            "A `schedule \"cron\" zone \"…\" -> job(args)` declaration \
             persists to the durable queue store and survives process \
             restart. Slice 35-N walks `Decl::Schedule` so a signed \
             cdylib that declares a cron schedule cannot ship without \
             this guarantee in its descriptor.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-driver/src/build/tests.rs::signed_claim_coverage_walks_schedule_decl",
        ],
        adversarial_test_refs: &[
            "crates/corvid-driver/src/build/tests.rs::signed_claim_coverage_rejects_schedule_without_jobs_coverage",
        ],
    },
    Guarantee {
        id: "jobs.retry_budget_bound",
        kind: GuaranteeKind::Jobs,
        class: GuaranteeClass::OutOfScope,
        phase: Phase::Runtime,
        description:
            "`@retry(max_attempts: N, backoff: ...)` bounds the runtime \
             retry envelope of a job so a transient failure cannot \
             escalate into unbounded re-spend.",
        out_of_scope_reason:
            "The runtime queue and lease envelopes are shipped, but \
             `@retry` is not yet a parser-level attribute. Slice 38K \
             promotes this row to `RuntimeChecked` when the multi-worker \
             runner consumes the attribute end-to-end.",
        positive_test_refs: &[],
        adversarial_test_refs: &[],
    },
    Guarantee {
        id: "jobs.idempotency_key_uniqueness",
        kind: GuaranteeKind::Jobs,
        class: GuaranteeClass::RuntimeChecked,
        phase: Phase::Runtime,
        description:
            "Across N concurrent workers, exactly one durable queue \
             row exists for a given non-null idempotency key. \
             Enforced by a partial UNIQUE INDEX on \
             `queue_jobs(idempotency_key) WHERE idempotency_key IS \
             NOT NULL` in the SQLite schema, plus the existing \
             `enqueue_typed_idempotent` collision-fallback path \
             that returns the surviving row when the insert hits \
             the UNIQUE constraint.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-runtime/src/queue/tests/durable_basics.rs::durable_queue_idempotency_key_collapses_duplicate_jobs",
        ],
        adversarial_test_refs: &[
            "crates/corvid-runtime/tests/durability_corpus.rs::t38l_d1_four_workers_collapse_to_one_row",
        ],
    },
    Guarantee {
        id: "jobs.lease_exclusivity",
        kind: GuaranteeKind::Jobs,
        class: GuaranteeClass::RuntimeChecked,
        phase: Phase::Runtime,
        description:
            "A job lease prevents two workers from running the same \
             job concurrently. Slice 38K's `WorkerPool` over \
             `DurableQueueRuntime` runs N tokio tasks each \
             contesting `lease_next_at`; the SQLite UPDATE that \
             flips `pending` → `leased` is atomic, so exactly one \
             worker wins each contention round. Lease expiry plus \
             a fresh worker re-leasing is shipped (slice 38L's D3 \
             test); heartbeat extension for long-running steps \
             remains a follow-up.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-runtime/src/worker_pool.rs::t38k_pool_runs_each_job_exactly_once",
        ],
        adversarial_test_refs: &[
            "crates/corvid-runtime/src/worker_pool.rs::t38k_two_workers_cannot_both_lease_same_job",
            "crates/corvid-runtime/src/worker_pool.rs::t38k_pool_drains_gracefully_without_claiming_new_work",
        ],
    },
    Guarantee {
        id: "jobs.durable_resume",
        kind: GuaranteeKind::Jobs,
        class: GuaranteeClass::RuntimeChecked,
        phase: Phase::Runtime,
        description:
            "A worker that drops uncleanly mid-step (the SIGKILL \
             surrogate the queue runtime is responsible for) leaves \
             behind durable checkpoint rows; a fresh worker that \
             opens the same SQLite file after the lease TTL elapses \
             can re-lease the job and resume from those checkpoints. \
             SQLite WAL fsync makes this property structural. The \
             count-bounded `no double LLM call` extension joins the \
             Phase 21 Replay corpus when step-skip semantics land at \
             the VM layer.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-runtime/src/queue/tests/checkpoints.rs::durable_queue_records_ordered_agent_checkpoints",
        ],
        adversarial_test_refs: &[
            "crates/corvid-runtime/tests/durability_corpus.rs::t38l_d3_checkpoints_survive_unclean_shutdown",
        ],
    },
    Guarantee {
        id: "jobs.cron_dst_correct",
        kind: GuaranteeKind::Jobs,
        class: GuaranteeClass::RuntimeChecked,
        phase: Phase::Runtime,
        description:
            "Cron schedules expressed in `America/New_York` (and \
             other DST-observing timezones) produce monotonic UTC \
             fire times across the spring-forward and fall-back \
             transitions, with no duplicates and no fire at the \
             non-existent local instant. `chrono-tz` is wired into \
             the queue runtime; the cron-crate's `Schedule::after` \
             iterator is timezone-aware.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-runtime/tests/durability_corpus.rs::t38l_d2_dst_spring_forward_is_deterministic",
        ],
        adversarial_test_refs: &[
            "crates/corvid-runtime/tests/durability_corpus.rs::t38l_d2_dst_fall_back_is_monotonic",
        ],
    },
    Guarantee {
        id: "jobs.approval_wait_resume",
        kind: GuaranteeKind::Jobs,
        class: GuaranteeClass::OutOfScope,
        phase: Phase::Runtime,
        description:
            "`await_approval` pauses a job until an approval token \
             arrives, expires, or is denied; the resume path writes \
             the audit transition.",
        out_of_scope_reason:
            "Runtime approval-wait state ships; `await_approval` is \
             not yet a parser-level keyword. Slice 38K (or a \
             follow-up syntax slice) promotes.",
        positive_test_refs: &[],
        adversarial_test_refs: &[],
    },
    Guarantee {
        id: "jobs.loop_bounds_enforced",
        kind: GuaranteeKind::Jobs,
        class: GuaranteeClass::OutOfScope,
        phase: Phase::Runtime,
        description:
            "Agent loops driven by jobs honor max-steps, max-wall-time, \
             max-spend, and max-tool-calls; exceeding any bound \
             escalates or terminates with trace evidence.",
        out_of_scope_reason:
            "Loop-bound envelopes ship; the multi-worker runner that \
             enforces them across crash + restart is not yet wired. \
             Slice 38K promotes.",
        positive_test_refs: &[],
        adversarial_test_refs: &[],
    },
    // ----- Auth (Phase 39) ---------------------------------------
    Guarantee {
        id: "auth.session_rotation_on_privilege_change",
        kind: GuaranteeKind::Auth,
        class: GuaranteeClass::OutOfScope,
        phase: Phase::Runtime,
        description:
            "A session id rotates on privilege escalation (role \
             upgrade, password change) so a stolen pre-escalation \
             cookie cannot exercise the post-escalation privilege.",
        out_of_scope_reason:
            "Session table ships; rotation hook is not yet wired \
             through a parser-level `auth` block. Slice 39L promotes.",
        positive_test_refs: &[],
        adversarial_test_refs: &[],
    },
    Guarantee {
        id: "auth.api_key_at_rest_hashed",
        kind: GuaranteeKind::Auth,
        class: GuaranteeClass::RuntimeChecked,
        phase: Phase::Runtime,
        description:
            "API keys are stored only as Argon2id hashes; the \
             plaintext leaves Corvid memory exactly once at issuance \
             and is never logged. Verified by the existing \
             `hash_api_key_secret`/`verify_api_key_secret` path in \
             `corvid-runtime/src/auth.rs`.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-runtime/src/auth/api_keys.rs::api_key_runtime_resolves_service_actor_with_argon2_hash_and_redacted_audit",
        ],
        adversarial_test_refs: &[
            "crates/corvid-runtime/src/auth/api_keys.rs::api_key_runtime_rejects_wrong_tenant_revoked_expired_and_user_actors",
        ],
    },
    Guarantee {
        id: "auth.jwt_kid_rotation",
        kind: GuaranteeKind::Auth,
        class: GuaranteeClass::RuntimeChecked,
        phase: Phase::Runtime,
        description:
            "JWT verification fetches the JWKS, picks the key by \
             `kid`, verifies the signature with `jsonwebtoken`, and \
             rejects tokens whose `kid` is missing from the current \
             JWKS, whose alg does not match the contract, whose \
             signature fails to verify, whose exp/iss/aud do not \
             align with the contract, or whose required \
             subject/tenant claim is missing. Out-of-scope at \
             Phase 39 base; promoted to `RuntimeChecked` by slice \
             39K when `corvid-runtime/src/jwt_verify/` shipped.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-runtime/src/jwt_verify/verifier.rs::parse_alg_accepts_supported_and_refuses_others",
            "crates/corvid-runtime/src/jwt_verify/verifier.rs::decoding_key_for_rsa_jwk_constructs",
            "crates/corvid-runtime/src/jwt_verify/mod.rs::error_slugs_are_stable_for_audit_log",
        ],
        adversarial_test_refs: &[
            "crates/corvid-runtime/src/jwt_verify/verifier.rs::kid_downgrade_returns_kid_not_found",
            "crates/corvid-runtime/src/jwt_verify/verifier.rs::header_alg_must_match_contract_alg",
            "crates/corvid-runtime/src/jwt_verify/verifier.rs::alg_none_in_header_is_refused",
            "crates/corvid-runtime/src/jwt_verify/verifier.rs::malformed_token_is_refused_before_fetch",
            "crates/corvid-runtime/src/jwt_verify/verifier.rs::jwks_fetch_failure_is_surfaced",
            "crates/corvid-runtime/src/jwt_verify/verifier.rs::decoding_key_for_rejects_rsa_without_n",
            "crates/corvid-runtime/src/jwt_verify/verifier.rs::decoding_key_for_rejects_unknown_kty",
        ],
    },
    Guarantee {
        id: "auth.oauth_pkce_required",
        kind: GuaranteeKind::Auth,
        class: GuaranteeClass::RuntimeChecked,
        phase: Phase::Runtime,
        description:
            "OAuth callback state requires PKCE for public clients; \
             the state record carries the code-verifier hash and is \
             single-use, tenant-scoped, and expiry-bound.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-runtime/src/auth/oauth.rs::oauth_callback_state_is_hashed_single_use_and_restart_safe",
        ],
        adversarial_test_refs: &[
            "crates/corvid-runtime/src/auth/oauth.rs::oauth_callback_rejects_expired_and_cross_tenant_state",
        ],
    },
    Guarantee {
        id: "auth.csrf_double_submit",
        kind: GuaranteeKind::Auth,
        class: GuaranteeClass::OutOfScope,
        phase: Phase::Runtime,
        description:
            "CSRF protection on cookie-bearing requests uses a \
             double-submit token verified by HMAC-SHA256.",
        out_of_scope_reason:
            "Token shape is documented in the design brief; the \
             middleware path that enforces it on every cookie-bearing \
             POST/PUT/PATCH/DELETE is not yet wired into the generated \
             axum server. Slice 39L promotes.",
        positive_test_refs: &[],
        adversarial_test_refs: &[],
    },
    Guarantee {
        id: "tenant.cross_tenant_compile_error",
        kind: GuaranteeKind::Auth,
        class: GuaranteeClass::OutOfScope,
        phase: Phase::TypeCheck,
        description:
            "A function whose actor came from tenant A may not pass \
             a record owned by tenant B to a tool that writes back \
             into A — the typechecker rejects the cross-tenant \
             reference.",
        out_of_scope_reason:
            "Tenant tagging exists in runtime envelopes but the \
             parser-level `tenant Org { ... }` block does not exist \
             yet. Slice 39L (parser surface) plus a typecheck slice \
             promotes this row to `Static`.",
        positive_test_refs: &[],
        adversarial_test_refs: &[],
    },
    Guarantee {
        id: "approval.policy_clause_static_check",
        kind: GuaranteeKind::Auth,
        class: GuaranteeClass::OutOfScope,
        phase: Phase::TypeCheck,
        description:
            "An `approval Name:` block's `policy { ... }` clause \
             type-checks at compile time so a malformed predicate \
             (wrong field name, wrong type, undeclared role) cannot \
             ship.",
        out_of_scope_reason:
            "Approval store ships; the `approval Name:` parser-level \
             block does not. Slice 39L promotes.",
        positive_test_refs: &[],
        adversarial_test_refs: &[],
    },
    Guarantee {
        id: "approval.batch_equivalence_typed",
        kind: GuaranteeKind::Auth,
        class: GuaranteeClass::OutOfScope,
        phase: Phase::TypeCheck,
        description:
            "An `approval ... batch_with: same_tool, same_data_class, \
             same_role` clause groups equivalent approvals so a \
             reviewer can approve one record and have N \
             equivalent-by-typed-shape records auto-resolve.",
        out_of_scope_reason:
            "Batch logic exists in the approval queue runtime but the \
             `batch_with` clause has no parser surface. Slice 39L \
             promotes.",
        positive_test_refs: &[],
        adversarial_test_refs: &[],
    },
    Guarantee {
        id: "approval.confused_deputy_typecheck",
        kind: GuaranteeKind::Auth,
        class: GuaranteeClass::OutOfScope,
        phase: Phase::TypeCheck,
        description:
            "A reachable path from any route or job to a `@dangerous` \
             tool must have an `approve` token whose `required_role` \
             covers every reachable caller — otherwise typecheck \
             rejects.",
        out_of_scope_reason:
            "Lexical-scope approval check ships (`approval.token_lexical_only`); \
             the cross-call reachability extension into routes/jobs \
             is not yet wired. Slice 39L promotes.",
        positive_test_refs: &[],
        adversarial_test_refs: &[],
    },
    // ----- Connector (Phase 41) ----------------------------------
    Guarantee {
        id: "connector.scope_minimum_enforced",
        kind: GuaranteeKind::Connector,
        class: GuaranteeClass::RuntimeChecked,
        phase: Phase::Runtime,
        description:
            "A connector cannot use a scope its manifest does not \
             declare and an actor cannot use a scope its auth state \
             does not authorise. The runtime fires \
             `ConnectorAuthError::MissingScope` (or `UnknownScope`) \
             before any HTTP layer touches the network, so a leaked \
             low-scope token cannot escalate to a higher-scope \
             operation by guessing the scope id.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-connector-runtime/src/runtime.rs::mock_mode_checks_auth_rate_limit_and_emits_trace",
        ],
        adversarial_test_refs: &[
            "crates/corvid-connector-runtime/tests/threat_corpus.rs::t1_github_rejects_unauthorised_scope",
            "crates/corvid-connector-runtime/tests/threat_corpus.rs::t1_gmail_rejects_unauthorised_scope",
            "crates/corvid-connector-runtime/tests/threat_corpus.rs::t1_slack_rejects_unauthorised_scope",
        ],
    },
    Guarantee {
        id: "connector.write_requires_approval",
        kind: GuaranteeKind::Connector,
        class: GuaranteeClass::OutOfScope,
        phase: Phase::TypeCheck,
        description:
            "A connector method whose effect set names a write \
             (`gmail.send`, `slack.post`, `github.create_issue`) \
             reaches typecheck only when its caller has a matching \
             `approve` boundary in lexical scope.",
        out_of_scope_reason:
            "Manifest declares write effects but the connector AST \
             surface is not yet parser-level — connectors today are \
             configured by Rust data, not source. Slice 41L \
             promotes.",
        positive_test_refs: &[],
        adversarial_test_refs: &[],
    },
    Guarantee {
        id: "connector.rate_limit_respects_provider",
        kind: GuaranteeKind::Connector,
        class: GuaranteeClass::RuntimeChecked,
        phase: Phase::Runtime,
        description:
            "A connector honors the provider's rate-limit advice \
             (`Retry-After`, 429, 5xx). The shared `ReqwestRealClient` \
             parses RFC 7231 `Retry-After` integer-seconds into \
             milliseconds via `parse_retry_after_header` and surfaces \
             it as `ConnectorRuntimeError::RateLimited { retry_after_ms }`, \
             which the runtime forwards verbatim to the caller \
             instead of retrying behind their back.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-connector-runtime/src/real_client.rs::parse_retry_after_seconds_form",
        ],
        adversarial_test_refs: &[
            "crates/corvid-connector-runtime/src/real_client.rs::parse_retry_after_returns_none_for_malformed",
            "crates/corvid-connector-runtime/src/runtime.rs::real_mode_propagates_rate_limited_from_bound_client",
            "crates/corvid-connector-runtime/tests/threat_corpus.rs::t5_rate_limited_propagates_retry_after_ms",
        ],
    },
    Guarantee {
        id: "connector.contract_drift_detected",
        kind: GuaranteeKind::Connector,
        class: GuaranteeClass::OutOfScope,
        phase: Phase::Runtime,
        description:
            "`corvid connectors check --live` compares the manifest \
             to the live (or recorded-cassette) provider response \
             shape and exits non-zero when fields drift.",
        out_of_scope_reason:
            "Slice 41L wired `corvid connectors check`, which validates \
             every shipped manifest against the manifest schema and \
             reports diagnostics per connector \
             (`shipped_manifests` → `validate_connector_manifest`). \
             The `--live` drift-narration path that compares the \
             manifest to a live provider response shape is gated \
             behind `CORVID_PROVIDER_LIVE=1` and currently returns \
             an explicit `Err` directing the caller to slice 41M-C; \
             until that slice ships, drift detection itself is not \
             exercised end-to-end and this row stays out of scope.",
        positive_test_refs: &[],
        adversarial_test_refs: &[],
    },
    Guarantee {
        id: "connector.webhook_signature_verified",
        kind: GuaranteeKind::Connector,
        class: GuaranteeClass::RuntimeChecked,
        phase: Phase::Runtime,
        description:
            "Inbound webhook payloads from Slack, GitHub, and Linear \
             are HMAC-SHA256 verified against the manifest's shared \
             secret before any handler runs. Per-provider schemes are \
             honored: GitHub uses `X-Hub-Signature-256: sha256=<hex>`, \
             Slack uses `v0:<ts>:<body>` with a 5-minute replay \
             window, and Linear uses a bare hex digest. Comparison is \
             constant-time; a malformed header, mismatched digest, or \
             stale Slack timestamp returns a categorical \
             `WebhookVerificationOutcome` that the dispatcher must \
             reject before any side effect.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-connector-runtime/src/webhook_verify.rs::github_verifies_correct_signature",
            "crates/corvid-connector-runtime/src/webhook_verify.rs::slack_verifies_correct_signature_inside_window",
            "crates/corvid-connector-runtime/src/webhook_verify.rs::linear_verifies_correct_signature",
        ],
        adversarial_test_refs: &[
            "crates/corvid-connector-runtime/tests/threat_corpus.rs::t7_github_webhook_forgery_rejected",
            "crates/corvid-connector-runtime/tests/threat_corpus.rs::t7_slack_webhook_replay_outside_window_rejected",
            "crates/corvid-connector-runtime/tests/threat_corpus.rs::t7_linear_webhook_wrong_secret_rejected",
        ],
    },
    Guarantee {
        id: "connector.replay_quarantine",
        kind: GuaranteeKind::Connector,
        class: GuaranteeClass::RuntimeChecked,
        phase: Phase::Runtime,
        description:
            "A connector running in replay mode must not perform \
             provider writes. The runtime returns \
             `ConnectorRuntimeError::ReplayWriteQuarantined` for any \
             scope whose effects include a `*.write` or `send_*` \
             effect when the active mode is `Replay`, regardless of \
             whether a real client is bound. Read-shaped operations \
             still complete from the recorded cassette so deterministic \
             replay continues to work.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-connector-runtime/src/test_kit.rs::fixture_runs_mock_and_replay_read_paths",
        ],
        adversarial_test_refs: &[
            "crates/corvid-connector-runtime/src/runtime.rs::replay_mode_quarantines_writes",
            "crates/corvid-connector-runtime/src/test_kit.rs::fixture_proves_replay_write_quarantine",
            "crates/corvid-connector-runtime/src/calendar.rs::calendar_replay_quarantines_writes",
            "crates/corvid-connector-runtime/src/slack.rs::slack_replay_quarantines_writes",
        ],
    },
    // ----- Observability (Phase 40) ------------------------------
    Guarantee {
        id: "observability.otel_conformance",
        kind: GuaranteeKind::Observability,
        class: GuaranteeClass::RuntimeChecked,
        phase: Phase::Runtime,
        description:
            "Lineage events flow through the standard \
             `opentelemetry` + `opentelemetry-otlp` SDK and emit \
             OTLP/HTTP spans whose attributes carry \
             `corvid.guarantee_id`, `corvid.cost_usd`, \
             `corvid.approval_id`, `corvid.replay_key`. The \
             attribute set is constructed by \
             `corvid_runtime::otel_sdk_export::corvid_span_attributes` \
             and the live wire path is exercised by the \
             docker-compose Jaeger harness in \
             `docs/observability-conformance.md`.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-runtime/src/otel_sdk_export.rs::span_attributes_include_corvid_named_keys",
            "crates/corvid-runtime/src/otel_sdk_export.rs::span_name_uses_corvid_prefix_with_kind",
            "crates/corvid-runtime/src/otel_sdk_export.rs::span_kind_maps_lineage_to_otel",
        ],
        adversarial_test_refs: &[
            "crates/corvid-runtime/src/otel_sdk_export.rs::span_attributes_omit_missing_optional_keys",
            "crates/corvid-runtime/src/otel_sdk_export.rs::sdk_exporter_reaches_in_process_otlp_receiver",
        ],
    },
    Guarantee {
        id: "observability.lineage_completeness",
        kind: GuaranteeKind::Observability,
        class: GuaranteeClass::RuntimeChecked,
        phase: Phase::Runtime,
        description:
            "Every lineage event carries a (trace_id, span_id) \
             pair plus parent linkage when a parent exists, so a \
             SQL JOIN against the local trace store reconstructs \
             the route → job → agent → prompt → tool → approval \
             → DB tree. Validated on every event via \
             `corvid_runtime::lineage::validate_lineage`.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-runtime/src/lineage.rs::lineage_ids_are_stable_and_parented_across_backend_kinds",
        ],
        adversarial_test_refs: &[
            "crates/corvid-runtime/src/lineage.rs::lineage_validation_fails_closed_for_missing_parent_or_duplicate_root",
        ],
    },
    Guarantee {
        id: "observability.redaction_determinism",
        kind: GuaranteeKind::Observability,
        class: GuaranteeClass::RuntimeChecked,
        phase: Phase::Runtime,
        description:
            "Redacting the same lineage event twice with the same \
             `LineageRedactionPolicy` yields byte-identical \
             output; trace topology (trace_id, span_id, parent \
             linkage) is preserved across redaction so observe / \
             eval / OTel keep correlating after sensitive values \
             are removed.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-runtime/src/lineage_redact.rs::redaction_preserves_topology_and_redacts_identifiers_deterministically",
        ],
        adversarial_test_refs: &[
            "crates/corvid-runtime/src/lineage_redact.rs::redaction_removes_obvious_secrets_from_serialized_lineage",
        ],
    },
    Guarantee {
        id: "observability.contract_aware_grouping",
        kind: GuaranteeKind::Observability,
        class: GuaranteeClass::RuntimeChecked,
        phase: Phase::Runtime,
        description:
            "`corvid observe show` groups incidents by \
             guarantee_id, effect, budget, provenance, and \
             approval rule rather than by service.name — so an \
             analyst's first pivot lands on the contract that \
             broke. Implemented by \
             `lineage_incidents::group_lineage_incidents`.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-runtime/src/lineage_incidents.rs::incidents_group_by_guarantee_effect_budget_provenance_and_approval",
        ],
        adversarial_test_refs: &[
            "crates/corvid-runtime/src/lineage_incidents.rs::non_incident_ok_events_are_not_grouped",
        ],
    },
    Guarantee {
        id: "eval.drift_attribution",
        kind: GuaranteeKind::Observability,
        class: GuaranteeClass::RuntimeChecked,
        phase: Phase::Runtime,
        description:
            "`corvid eval-drift --explain` decomposes the drift \
             between two trace runs into the four named \
             dimensions (model_id, prompt_hash, \
             retrieval_index_hash, input_fingerprint) plus a \
             residual percentage for unattributable changes. The \
             output's `sources` array carries the trace_id + \
             span_id of every event the analysis consulted.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-cli/src/observe_helpers_cmd/eval_drift.rs::drift_explain_attributes_model_swap",
        ],
        adversarial_test_refs: &[
            "crates/corvid-cli/src/observe_helpers_cmd/eval_drift.rs::drift_explain_surfaces_residual_when_status_flips_alone",
        ],
    },
    Guarantee {
        id: "eval.promotion_signed_lineage",
        kind: GuaranteeKind::Observability,
        class: GuaranteeClass::RuntimeChecked,
        phase: Phase::Runtime,
        description:
            "`corvid eval-from-feedback` synthesises a typed \
             eval fixture from a 'wrong answer' feedback record, \
             redacting the matching lineage trace via the \
             production redaction policy before writing the \
             fixture. The fixture's `sources` field lists every \
             redacted event so downstream consumers can \
             reconstruct evidence without seeing raw identifiers.",
        out_of_scope_reason: "",
        positive_test_refs: &[
            "crates/corvid-cli/src/observe_helpers_cmd/eval_from_feedback.rs::eval_generate_from_feedback_writes_redacted_fixture",
        ],
        adversarial_test_refs: &[
            "crates/corvid-cli/src/observe_helpers_cmd/eval_from_feedback.rs::eval_generate_from_feedback_missing_trace_id_refused",
        ],
    },
    Guarantee {
        id: "review_queue.cost_of_being_wrong_ranking",
        kind: GuaranteeKind::Observability,
        class: GuaranteeClass::OutOfScope,
        phase: Phase::Runtime,
        description:
            "`corvid review-queue list --rank=cost-of-being-wrong` \
             surfaces low-confidence + high-risk outputs ranked \
             by the `cost_of_being_wrong` policy.",
        out_of_scope_reason:
            "Review-queue envelopes ship at `corvid_runtime::review_queue`; \
             the ranking CLI subcommand is not yet wired. A \
             follow-up slice promotes this row when \
             `corvid review-queue list` lands.",
        positive_test_refs: &[],
        adversarial_test_refs: &[],
    },
    // ----- Platform: explicit non-defenses ------------------------
    Guarantee {
        id: "platform.host_kernel_compromise",
        kind: GuaranteeKind::Platform,
        class: GuaranteeClass::OutOfScope,
        phase: Phase::Platform,
        description:
            "Defending against a compromised host kernel or \
             privileged-process tampering with the running Corvid \
             binary's memory.",
        out_of_scope_reason:
            "Outside Corvid's trust boundary — a kernel that can rewrite \
             user-space memory can defeat any user-space invariant. The \
             security model assumes a non-malicious kernel; otherwise \
             the host is responsible.",
        positive_test_refs: &[],
        adversarial_test_refs: &[],
    },
    Guarantee {
        id: "platform.signing_key_compromise",
        kind: GuaranteeKind::Platform,
        class: GuaranteeClass::OutOfScope,
        phase: Phase::Platform,
        description:
            "Defending against compromise of the ed25519 signing key used \
             to attest a cdylib or sign a receipt.",
        out_of_scope_reason:
            "Key management is a host responsibility. Corvid signs and \
             verifies; rotating, revoking, and protecting keys is \
             outside the language's scope and explicitly delegated to \
             the host's key-management practice.",
        positive_test_refs: &[],
        adversarial_test_refs: &[],
    },
    Guarantee {
        id: "platform.toolchain_compromise",
        kind: GuaranteeKind::Platform,
        class: GuaranteeClass::OutOfScope,
        phase: Phase::Platform,
        description:
            "Defending against a compromised Rust toolchain, Cranelift \
             release, or system linker producing a Corvid binary that \
             does not match its source.",
        out_of_scope_reason:
            "Reproducible builds across heterogeneous toolchains are a \
             post-v1.0 hardening goal. Today Corvid trusts the rustc and \
             Cranelift releases the user installs; the bilateral verifier \
             (Slice 35-H) is the closest approximation of \
             toolchain-independence available pre-v1.0.",
        positive_test_refs: &[],
        adversarial_test_refs: &[],
    },
    Guarantee {
        id: "package.hosted_registry_available",
        kind: GuaranteeKind::Platform,
        class: GuaranteeClass::OutOfScope,
        phase: Phase::Platform,
        description:
            "A Corvid-operated public package registry service that \
             serves the published index format and source artifacts.",
        out_of_scope_reason:
            "No hosted Corvid package registry service runs yet; \
             The CLI ships the published index format + signed-publish \
             tooling (`corvid package publish`, `verify-registry`, \
             `verify-lock`) and `--url-base` accepts file:// and any \
             http endpoint a user runs themselves. A hosted public \
             registry is post-v1.0 work; see `docs/package-manager-scope.md` \
             for the full boundary.",
        positive_test_refs: &[],
        adversarial_test_refs: &[],
    },
];

pub fn lookup(id: &str) -> Option<&'static Guarantee> {
    GUARANTEE_REGISTRY.iter().find(|g| g.id == id)
}

/// Iterate every guarantee in declaration order.
pub fn iter() -> impl Iterator<Item = &'static Guarantee> {
    GUARANTEE_REGISTRY.iter()
}

/// Iterate guarantees of a given class in declaration order.
pub fn by_class(class: GuaranteeClass) -> impl Iterator<Item = &'static Guarantee> {
    GUARANTEE_REGISTRY.iter().filter(move |g| g.class == class)
}

/// Iterate guarantees of a given kind in declaration order.
pub fn by_kind(kind: GuaranteeKind) -> impl Iterator<Item = &'static Guarantee> {
    GUARANTEE_REGISTRY.iter().filter(move |g| g.kind == kind)
}
