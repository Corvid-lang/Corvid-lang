//! Honesty-rule validation for [`super::GUARANTEE_REGISTRY`] —
//! slice 35-A / canonical contract surface, decomposed in Phase
//! 20j-A8.
//!
//! Two consumers run [`validate_slice`]:
//!
//! - The in-crate test `registry_is_well_formed` asserts the
//!   shipped `GUARANTEE_REGISTRY` is honest at build time.
//! - `corvid contract list --validate` runs the same check at
//!   the CLI surface for any registry slice the caller supplies.
//!
//! The honesty rules enforced here:
//!
//! - No two rows share an id (`DuplicateId`).
//! - Every id is `<kind>.<specific>` with both halves non-empty
//!   and `[a-z0-9_]`-only (`MalformedId`).
//! - Every `OutOfScope` row carries an `out_of_scope_reason`
//!   (`OutOfScopeMissingReason`).
//! - No `Static` / `RuntimeChecked` row carries an
//!   `out_of_scope_reason` (`EnforcedHasReason`) — that would
//!   mean a row claiming both "we enforce this" and "we don't".
//! - Every row has a non-empty `description`
//!   (`EmptyDescription`).
//! - No row has an empty id (`EmptyId`).

use std::fmt;

use super::types::{Guarantee, GuaranteeClass};

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
