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

pub mod types;
pub mod validate;
pub use types::*;
pub use validate::*;

pub mod registry;
pub use registry::*;


pub mod signed_claim;
pub use signed_claim::*;


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_well_formed() {
        validate_slice(GUARANTEE_REGISTRY).expect("registry well-formed");
    }

    #[test]
    fn lookup_finds_known_entry() {
        let g = lookup("approval.dangerous_call_requires_token").expect("entry exists");
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
    fn signed_cdylib_claim_ids_resolve_to_enforced_guarantees() {
        let mut seen = std::collections::BTreeSet::new();
        for id in SIGNED_CDYLIB_CLAIM_GUARANTEE_IDS {
            assert!(seen.insert(*id), "duplicate signed cdylib claim id `{id}`");
            let guarantee = lookup(id)
                .unwrap_or_else(|| panic!("signed cdylib claim id `{id}` is not registered"));
            assert_ne!(
                guarantee.class,
                GuaranteeClass::OutOfScope,
                "signed cdylib claim id `{id}` must be enforced"
            );
        }
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
        let entries = [GUARANTEE_REGISTRY[0], GUARANTEE_REGISTRY[0]];
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
            for r in g
                .positive_test_refs
                .iter()
                .chain(g.adversarial_test_refs.iter())
            {
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
            for r in g
                .positive_test_refs
                .iter()
                .chain(g.adversarial_test_refs.iter())
            {
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
