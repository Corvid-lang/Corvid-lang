//! Canonical registry of every public Corvid guarantee.
//!
//! This crate is the single source of truth for what Corvid promises,
//! who enforces it, and where in the pipeline that enforcement lives.
//! Every later Phase 35 artifact derives from this registry:
//!
//!   * `corvid contract list` prints the registry.
//!   * `docs/core-semantics.md` is generated from it.
//!   * The bilateral verifier cross-checks against it.
//!   * `corvid claim --explain` reports per-binary which entries
//!     were enforced.
//!   * `corvid build --sign` refuses to ship unless every declared
//!     contract maps to a registry entry.
//!
//! No public guarantee is anonymous. If a check exists in the
//! compiler or runtime that backs a public claim, it must register
//! here. If a behaviour is documented but not enforced, it must
//! register here as `GuaranteeClass::OutOfScope` with an explicit
//! `out_of_scope_reason` — that is how the registry stays honest.

#![forbid(unsafe_code)]

pub mod render;

pub use render::render_core_semantics_markdown;

use std::fmt;

/// What kind of contract a guarantee enforces.
///
/// Kinds are coarse categories matching Corvid's moat dimensions:
/// approval boundaries, effect rows, grounding, cost budgets,
/// confidence thresholds, replay determinism, provenance, and the
/// ABI surface. Every guarantee in the registry belongs to exactly
/// one kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GuaranteeKind {
    Approval,
    EffectRow,
    Grounded,
    Budget,
    Confidence,
    Replay,
    ProvenanceTrace,
    AbiDescriptor,
    AbiAttestation,
    Platform,
}

impl GuaranteeKind {
    /// Stable lowercase slug used by JSON serialisation, doc
    /// generation, and CLI output. Slugs MUST stay stable across
    /// versions — they appear in `corvid claim --explain` output
    /// that downstream tooling parses.
    pub const fn slug(self) -> &'static str {
        match self {
            GuaranteeKind::Approval => "approval",
            GuaranteeKind::EffectRow => "effect_row",
            GuaranteeKind::Grounded => "grounded",
            GuaranteeKind::Budget => "budget",
            GuaranteeKind::Confidence => "confidence",
            GuaranteeKind::Replay => "replay",
            GuaranteeKind::ProvenanceTrace => "provenance_trace",
            GuaranteeKind::AbiDescriptor => "abi_descriptor",
            GuaranteeKind::AbiAttestation => "abi_attestation",
            GuaranteeKind::Platform => "platform",
        }
    }

    pub const ALL: &'static [GuaranteeKind] = &[
        GuaranteeKind::Approval,
        GuaranteeKind::EffectRow,
        GuaranteeKind::Grounded,
        GuaranteeKind::Budget,
        GuaranteeKind::Confidence,
        GuaranteeKind::Replay,
        GuaranteeKind::ProvenanceTrace,
        GuaranteeKind::AbiDescriptor,
        GuaranteeKind::AbiAttestation,
        GuaranteeKind::Platform,
    ];
}

impl fmt::Display for GuaranteeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.slug())
    }
}

/// Strength of the enforcement promise.
///
/// `Static` means the compiler refuses to produce a binary when the
/// invariant would be violated; the user sees a diagnostic, not a
/// runtime crash. `RuntimeChecked` means the runtime detects the
/// violation and either terminates or reports through the documented
/// channel (a non-zero exit, a specific error variant, or a refused
/// operation). `OutOfScope` is a documented promise that does NOT
/// have a check today — every `OutOfScope` entry MUST carry a
/// non-empty `out_of_scope_reason` so that the registry remains an
/// honest list of what we do and do not defend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GuaranteeClass {
    Static,
    RuntimeChecked,
    OutOfScope,
}

impl GuaranteeClass {
    pub const fn slug(self) -> &'static str {
        match self {
            GuaranteeClass::Static => "static",
            GuaranteeClass::RuntimeChecked => "runtime_checked",
            GuaranteeClass::OutOfScope => "out_of_scope",
        }
    }

    pub const ALL: &'static [GuaranteeClass] = &[
        GuaranteeClass::Static,
        GuaranteeClass::RuntimeChecked,
        GuaranteeClass::OutOfScope,
    ];
}

impl fmt::Display for GuaranteeClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.slug())
    }
}

/// Pipeline stage that owns the enforcement.
///
/// The registry uses these as a coarse taxonomy so an outsider can
/// answer "where in the build is this checked?" The slugs match
/// Corvid's actual crate layout where possible (`resolve`,
/// `typecheck`, `ir_lower`, `codegen`, `runtime`, `abi_emit`,
/// `abi_verify`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Phase {
    Lex,
    Parse,
    Resolve,
    TypeCheck,
    IrLower,
    Codegen,
    Runtime,
    AbiEmit,
    AbiVerify,
    Platform,
}

impl Phase {
    pub const fn slug(self) -> &'static str {
        match self {
            Phase::Lex => "lex",
            Phase::Parse => "parse",
            Phase::Resolve => "resolve",
            Phase::TypeCheck => "typecheck",
            Phase::IrLower => "ir_lower",
            Phase::Codegen => "codegen",
            Phase::Runtime => "runtime",
            Phase::AbiEmit => "abi_emit",
            Phase::AbiVerify => "abi_verify",
            Phase::Platform => "platform",
        }
    }

    pub const ALL: &'static [Phase] = &[
        Phase::Lex,
        Phase::Parse,
        Phase::Resolve,
        Phase::TypeCheck,
        Phase::IrLower,
        Phase::Codegen,
        Phase::Runtime,
        Phase::AbiEmit,
        Phase::AbiVerify,
        Phase::Platform,
    ];
}

impl fmt::Display for Phase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.slug())
    }
}

/// One row of the canonical guarantee table.
///
/// Every field is `&'static` so the entire registry is built at
/// compile time and shared across every dependent crate without
/// allocation. The `id` is the stable handle referenced by
/// diagnostics, tests, generated docs, and the CLI; renaming an `id`
/// is a breaking change to the public claim surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Guarantee {
    /// Stable identifier of the form `kind.specific_promise`.
    /// Slug-cased, dot-separated, lowercase. Referenced by
    /// diagnostics and `corvid claim --explain` output.
    pub id: &'static str,
    pub kind: GuaranteeKind,
    pub class: GuaranteeClass,
    pub phase: Phase,
    /// One-line human description suitable for inclusion in
    /// generated docs and CLI output. Must remain accurate as the
    /// implementation evolves; the doc generator (Slice 35-D)
    /// emits this verbatim.
    pub description: &'static str,
    /// Why a guarantee is `OutOfScope`. Empty for `Static` and
    /// `RuntimeChecked`; non-empty (and validated) for `OutOfScope`.
    pub out_of_scope_reason: &'static str,
    /// Test functions that demonstrate the guarantee holds for
    /// valid programs. Slice 35-E enforces non-empty for `Static`
    /// and `RuntimeChecked` entries.
    pub positive_test_refs: &'static [&'static str],
    /// Test functions that demonstrate the guarantee rejects
    /// violations. Slice 35-E enforces non-empty for `Static` and
    /// `RuntimeChecked` entries.
    pub adversarial_test_refs: &'static [&'static str],
}

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
        class: GuaranteeClass::OutOfScope,
        phase: Phase::Resolve,
        description:
            "A `@dangerous` marker cannot be erased by re-exporting or \
             aliasing the tool through another module — every public \
             alias preserves the original danger annotation.",
        out_of_scope_reason:
            "Cross-module re-export / aliasing of dangerous tools is \
             enforced today only implicitly through symbol propagation \
             in the resolver — there is no dedicated diagnostic site \
             distinct from `approval.dangerous_call_requires_token`. \
             Slice 35-G's source-level bypass fuzz corpus will add an \
             explicit re-export-bypass mutator that exercises the \
             marker preservation path; once that lands this entry can \
             be promoted back to `Static` with concrete tests.",
        positive_test_refs: &[],
        adversarial_test_refs: &[],
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
];

/// Look up a guarantee by its stable id.
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

/// Reasons a registry row can fail validation. Every variant
/// represents an honesty rule the registry must keep — duplicates,
/// malformed ids, or `OutOfScope` rows without a stated reason all
/// erode the registry's value as a single source of truth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    DuplicateId(&'static str),
    EmptyId,
    MalformedId {
        id: &'static str,
        reason: &'static str,
    },
    OutOfScopeMissingReason(&'static str),
    EnforcedHasReason(&'static str),
    EmptyDescription(&'static str),
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegistryError::DuplicateId(id) => {
                write!(f, "duplicate guarantee id `{id}` in registry")
            }
            RegistryError::EmptyId => f.write_str("registry entry has empty id"),
            RegistryError::MalformedId { id, reason } => {
                write!(f, "guarantee id `{id}` is malformed: {reason}")
            }
            RegistryError::OutOfScopeMissingReason(id) => write!(
                f,
                "guarantee `{id}` is OutOfScope but has no out_of_scope_reason"
            ),
            RegistryError::EnforcedHasReason(id) => write!(
                f,
                "guarantee `{id}` is enforced (Static/RuntimeChecked) but \
                 carries an out_of_scope_reason — drop it or downgrade the class"
            ),
            RegistryError::EmptyDescription(id) => {
                write!(f, "guarantee `{id}` has empty description")
            }
        }
    }
}

impl std::error::Error for RegistryError {}

/// Validate an arbitrary slice of guarantees against the registry's
/// honesty rules. Used by the in-crate test that enforces these
/// rules on `GUARANTEE_REGISTRY` and re-used by the `corvid contract
/// list --validate` command in Slice 35-C.
pub fn validate_slice(entries: &[Guarantee]) -> Result<(), RegistryError> {
    let mut seen_ids = std::collections::HashSet::new();
    for g in entries {
        if g.id.is_empty() {
            return Err(RegistryError::EmptyId);
        }
        validate_id_shape(g.id)?;
        if g.description.trim().is_empty() {
            return Err(RegistryError::EmptyDescription(g.id));
        }
        match g.class {
            GuaranteeClass::OutOfScope => {
                if g.out_of_scope_reason.trim().is_empty() {
                    return Err(RegistryError::OutOfScopeMissingReason(g.id));
                }
            }
            GuaranteeClass::Static | GuaranteeClass::RuntimeChecked => {
                if !g.out_of_scope_reason.is_empty() {
                    return Err(RegistryError::EnforcedHasReason(g.id));
                }
            }
        }
        if !seen_ids.insert(g.id) {
            return Err(RegistryError::DuplicateId(g.id));
        }
    }
    Ok(())
}

fn validate_id_shape(id: &'static str) -> Result<(), RegistryError> {
    let mut parts = id.split('.');
    let prefix = parts.next().unwrap_or("");
    let suffix = parts.next().unwrap_or("");
    if parts.next().is_some() {
        return Err(RegistryError::MalformedId {
            id,
            reason: "expected exactly one '.' separating kind-prefix and specific-promise",
        });
    }
    if prefix.is_empty() || suffix.is_empty() {
        return Err(RegistryError::MalformedId {
            id,
            reason: "both prefix and suffix around '.' must be non-empty",
        });
    }
    for (label, part) in [("prefix", prefix), ("suffix", suffix)] {
        let mut chars = part.chars();
        let first = chars.next();
        match first {
            Some(c) if c.is_ascii_lowercase() => {}
            _ => {
                return Err(RegistryError::MalformedId {
                    id,
                    reason: if label == "prefix" {
                        "prefix must start with an ascii lowercase letter"
                    } else {
                        "suffix must start with an ascii lowercase letter"
                    },
                });
            }
        }
        for c in chars {
            if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
                return Err(RegistryError::MalformedId {
                    id,
                    reason: "id segments may contain only [a-z0-9_]",
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_well_formed() {
        validate_slice(GUARANTEE_REGISTRY).expect("registry well-formed");
    }

    #[test]
    fn lookup_finds_known_entry() {
        let g = lookup("approval.dangerous_call_requires_token")
            .expect("entry exists");
        assert_eq!(g.kind, GuaranteeKind::Approval);
        assert_eq!(g.class, GuaranteeClass::Static);
    }

    #[test]
    fn lookup_misses_unknown_entry() {
        assert!(lookup("nope.does_not_exist").is_none());
    }

    #[test]
    fn by_class_static_excludes_out_of_scope() {
        for g in by_class(GuaranteeClass::Static) {
            assert_ne!(g.class, GuaranteeClass::OutOfScope);
        }
        let static_count = by_class(GuaranteeClass::Static).count();
        assert!(
            static_count >= 5,
            "expected at least 5 static guarantees in seed, got {static_count}"
        );
    }

    #[test]
    fn by_kind_partitions_registry() {
        let mut total = 0;
        for kind in GuaranteeKind::ALL {
            total += by_kind(*kind).count();
        }
        assert_eq!(
            total,
            GUARANTEE_REGISTRY.len(),
            "every entry must belong to exactly one kind"
        );
    }

    #[test]
    fn out_of_scope_entries_carry_reasons() {
        let mut found = 0;
        for g in by_class(GuaranteeClass::OutOfScope) {
            assert!(
                !g.out_of_scope_reason.trim().is_empty(),
                "OutOfScope guarantee `{}` has no reason",
                g.id
            );
            found += 1;
        }
        assert!(
            found >= 1,
            "registry should explicitly enumerate at least one OutOfScope honest non-defense"
        );
    }

    #[test]
    fn duplicate_id_rejected() {
        let entries = [
            GUARANTEE_REGISTRY[0],
            GUARANTEE_REGISTRY[0],
        ];
        let err = validate_slice(&entries).expect_err("duplicate must fail");
        assert!(matches!(err, RegistryError::DuplicateId(_)));
    }

    #[test]
    fn out_of_scope_without_reason_rejected() {
        let bad = Guarantee {
            id: "test.no_reason",
            kind: GuaranteeKind::Platform,
            class: GuaranteeClass::OutOfScope,
            phase: Phase::Platform,
            description: "demo",
            out_of_scope_reason: "",
            positive_test_refs: &[],
            adversarial_test_refs: &[],
        };
        let err = validate_slice(&[bad]).expect_err("missing reason must fail");
        assert!(matches!(err, RegistryError::OutOfScopeMissingReason(_)));
    }

    #[test]
    fn enforced_with_reason_rejected() {
        let bad = Guarantee {
            id: "test.spurious_reason",
            kind: GuaranteeKind::Approval,
            class: GuaranteeClass::Static,
            phase: Phase::TypeCheck,
            description: "demo",
            out_of_scope_reason: "should not be set",
            positive_test_refs: &[],
            adversarial_test_refs: &[],
        };
        let err = validate_slice(&[bad]).expect_err("enforced + reason must fail");
        assert!(matches!(err, RegistryError::EnforcedHasReason(_)));
    }

    #[test]
    fn malformed_id_rejected() {
        let bad = Guarantee {
            id: "NoDot",
            kind: GuaranteeKind::Approval,
            class: GuaranteeClass::Static,
            phase: Phase::TypeCheck,
            description: "demo",
            out_of_scope_reason: "",
            positive_test_refs: &[],
            adversarial_test_refs: &[],
        };
        let err = validate_slice(&[bad]).expect_err("malformed id must fail");
        assert!(matches!(err, RegistryError::MalformedId { .. }));
    }

    #[test]
    fn slugs_round_trip_through_display() {
        for kind in GuaranteeKind::ALL {
            assert_eq!(format!("{kind}"), kind.slug());
        }
        for class in GuaranteeClass::ALL {
            assert_eq!(format!("{class}"), class.slug());
        }
        for phase in Phase::ALL {
            assert_eq!(format!("{phase}"), phase.slug());
        }
    }

    // ----------------------------------------------------------------
    // Phase 35-E: cross-reference enforcement.
    //
    // Every Static / RuntimeChecked guarantee must have at least one
    // positive test ref AND at least one adversarial test ref. Every
    // populated test ref must follow the format
    // `<file_path>::<fn_name>` and refer to a function that actually
    // exists in the named file.
    //
    // OutOfScope guarantees are exempt from the test-ref requirement
    // — they are explicit non-defenses; the `out_of_scope_reason` is
    // their proof. Slice 35-A's `validate_slice` already enforces
    // that exemption is honest.
    // ----------------------------------------------------------------

    fn split_test_ref(test_ref: &str) -> Option<(&str, &str)> {
        let mut parts = test_ref.rsplitn(2, "::");
        let fn_name = parts.next()?;
        let file_path = parts.next()?;
        if file_path.is_empty() || fn_name.is_empty() {
            return None;
        }
        Some((file_path, fn_name))
    }

    /// Read the file at `file_path` (interpreted relative to the
    /// workspace root, which is the `corvid-guarantees` crate's
    /// great-grandparent dir during tests).
    fn read_file_under_workspace(file_path: &str) -> Result<String, String> {
        // CARGO_MANIFEST_DIR is .../crates/corvid-guarantees during
        // `cargo test`. Walk up two levels to hit the workspace root.
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let workspace_root = manifest_dir
            .parent()
            .and_then(|p| p.parent())
            .ok_or_else(|| {
                format!(
                    "could not derive workspace root from CARGO_MANIFEST_DIR `{}`",
                    manifest_dir.display()
                )
            })?;
        let abs = workspace_root.join(file_path);
        std::fs::read_to_string(&abs).map_err(|e| {
            format!(
                "could not read `{}` (resolved to `{}`): {e}",
                file_path,
                abs.display()
            )
        })
    }

    #[test]
    fn every_enforced_guarantee_has_positive_and_adversarial_test_refs() {
        let mut missing: Vec<String> = Vec::new();
        for g in GUARANTEE_REGISTRY {
            if g.class == GuaranteeClass::OutOfScope {
                continue;
            }
            if g.positive_test_refs.is_empty() {
                missing.push(format!(
                    "guarantee `{}` (class {}) has zero positive_test_refs",
                    g.id,
                    g.class.slug()
                ));
            }
            if g.adversarial_test_refs.is_empty() {
                missing.push(format!(
                    "guarantee `{}` (class {}) has zero adversarial_test_refs",
                    g.id,
                    g.class.slug()
                ));
            }
        }
        assert!(
            missing.is_empty(),
            "phase 35-E test-coverage gap:\n  - {}\n\nEither downgrade the \
             guarantee to OutOfScope with an explicit reason or add tests \
             before promoting it back.",
            missing.join("\n  - ")
        );
    }

    #[test]
    fn every_test_ref_has_well_formed_path() {
        let mut malformed: Vec<String> = Vec::new();
        for g in GUARANTEE_REGISTRY {
            for r in g.positive_test_refs.iter().chain(g.adversarial_test_refs.iter()) {
                if split_test_ref(r).is_none() {
                    malformed.push(format!(
                        "guarantee `{}`: test_ref `{}` is not in `<file>::<fn>` form",
                        g.id, r
                    ));
                }
            }
        }
        assert!(
            malformed.is_empty(),
            "phase 35-E malformed test refs:\n  - {}",
            malformed.join("\n  - ")
        );
    }

    #[test]
    fn every_test_ref_resolves_to_a_real_test_function() {
        // Group refs by file so each file is read once.
        use std::collections::BTreeMap;
        let mut by_file: BTreeMap<&'static str, Vec<(&'static str, &'static str)>> =
            BTreeMap::new();
        for g in GUARANTEE_REGISTRY {
            for r in g.positive_test_refs.iter().chain(g.adversarial_test_refs.iter()) {
                let (file, func) = split_test_ref(r).expect(
                    "every_test_ref_has_well_formed_path enforces the shape; \
                     this should already pass before reaching here",
                );
                by_file.entry(file).or_default().push((g.id, func));
            }
        }

        let mut missing: Vec<String> = Vec::new();
        for (file, refs) in &by_file {
            let body = match read_file_under_workspace(file) {
                Ok(s) => s,
                Err(e) => {
                    for (gid, func) in refs {
                        missing.push(format!(
                            "guarantee `{gid}`: cannot read `{file}` to verify \
                             `{func}` exists ({e})"
                        ));
                    }
                    continue;
                }
            };
            for (gid, func) in refs {
                let needle = format!("fn {func}(");
                if !body.contains(&needle) {
                    missing.push(format!(
                        "guarantee `{gid}`: test function `{func}` not found in `{file}` \
                         (looked for literal `{needle}`)"
                    ));
                }
            }
        }
        assert!(
            missing.is_empty(),
            "phase 35-E unresolved test refs:\n  - {}",
            missing.join("\n  - ")
        );
    }
}
