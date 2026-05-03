//! Normal-form algebra for stacked trace-diff deltas.

use std::collections::HashMap;

use super::{build_anomaly, Anomaly, AnomalyClass, AnomalySeverity, DeltaRecord, StackDelta};

/// Running state of a Class A lifecycle chain across the stack.
pub(super) struct LifecycleChain {
    /// +1 = currently "gained/added" net of base; -1 = "lost/
    /// removed" net of base; 0 = cancelled to identity.
    state: i32,
    /// Polarity of the most recent delta seen — used to detect
    /// same-direction duplicates (anomaly) before updating state.
    last_polarity: i32,
    /// Last delta that moved the state. Becomes the normal-form
    /// emission when the chain survives.
    last_record: DeltaRecord,
    /// Commit that produced `last_record`.
    last_commit: String,
}

impl LifecycleChain {
    pub(super) fn into_normal_form(self) -> Option<StackDelta> {
        if self.state == 0 {
            None
        } else {
            Some(StackDelta {
                key: self.last_record.key,
                summary: self.last_record.summary,
                introduced_at: self.last_commit,
            })
        }
    }
}

/// Running state of a Class B transition chain.
pub(super) struct TransitionChain {
    initial_from: String,
    current_to: String,
    last_record: DeltaRecord,
    last_commit: String,
    /// Family + entity carried along so we can synthesize the
    /// net-transition key in `into_normal_form` when the chain
    /// composed across multiple commits (e.g. A→B then B→C must
    /// emit as `...:A->C`, not as the last per-commit key which
    /// encoded `B→C`).
    family: String,
    entity: String,
}

impl TransitionChain {
    pub(super) fn into_normal_form(self) -> Option<StackDelta> {
        if self.initial_from == self.current_to {
            return None;
        }
        let key = format!(
            "{}_changed:{}:{}->{}",
            self.family, self.entity, self.initial_from, self.current_to
        );
        let summary = format!(
            "net transition on `{}` over stack: `{}` → `{}`",
            self.entity, self.initial_from, self.current_to
        );
        Some(StackDelta {
            key,
            summary,
            introduced_at: self.last_commit,
        })
    }
}

pub(super) enum DeltaKind {
    Lifecycle {
        family: String,
        entity: String,
        polarity: i32,
    },
    Transition {
        family: String,
        entity: String,
        from: String,
        to: String,
    },
}

/// Classify a delta key into its algebraic role. Returns `None`
/// for keys the composer doesn't recognize — those flow through
/// history only and don't participate in normal-form composition.
pub(super) fn parse_delta_key(key: &str) -> Option<DeltaKind> {
    // Class B (transitions) first — the `_changed:` marker is
    // distinctive. Rest of the key is `<entity>:<from>-><to>`.
    if let Some((prefix, rest)) = key.split_once("_changed:") {
        let (entity, transition) = rest.rsplit_once(':')?;
        let (from, to) = transition.split_once("->")?;
        return Some(DeltaKind::Transition {
            family: prefix.to_string(),
            entity: entity.to_string(),
            from: from.to_string(),
            to: to.to_string(),
        });
    }

    // Class A lifecycle with `_<direction>:` suffix families. Each
    // entry is (suffix, polarity). Order doesn't matter across
    // entries because no pair is a substring of another.
    for (suffix, polarity) in &[
        ("_gained:", 1),
        ("_lost:", -1),
        ("_added:", 1),
        ("_removed:", -1),
    ] {
        if let Some((prefix, rest)) = key.split_once(suffix) {
            return Some(DeltaKind::Lifecycle {
                family: prefix.to_string(),
                entity: rest.to_string(),
                polarity: *polarity,
            });
        }
    }

    // `agent.added:X` / `agent.removed:X` are the one exception to
    // the suffix pattern — dot-separated instead of underscore-
    // joined. Map both to a synthetic `agent.lifecycle` family so
    // they cancel properly against each other.
    if let Some(entity) = key.strip_prefix("agent.added:") {
        return Some(DeltaKind::Lifecycle {
            family: "agent.lifecycle".to_string(),
            entity: entity.to_string(),
            polarity: 1,
        });
    }
    if let Some(entity) = key.strip_prefix("agent.removed:") {
        return Some(DeltaKind::Lifecycle {
            family: "agent.lifecycle".to_string(),
            entity: entity.to_string(),
            polarity: -1,
        });
    }

    None
}

pub(super) fn apply_lifecycle(
    state: &mut HashMap<(String, String), LifecycleChain>,
    anomalies: &mut Vec<Anomaly>,
    family: String,
    entity: String,
    polarity: i32,
    delta: &DeltaRecord,
    commit_sha: &str,
) {
    let key = (family.clone(), entity.clone());
    match state.get_mut(&key) {
        None => {
            state.insert(
                key,
                LifecycleChain {
                    state: polarity,
                    last_polarity: polarity,
                    last_record: delta.clone(),
                    last_commit: commit_sha.to_string(),
                },
            );
        }
        Some(chain) => {
            if chain.last_polarity == polarity {
                // Same direction twice — anomaly. Don't update
                // state: the first occurrence already pinned the
                // semantic state; the second is redundant noise
                // the algebra can't interpret.
                anomalies.push(build_anomaly(
                    AnomalyClass::SameDirectionDuplicate,
                    AnomalySeverity::Surface,
                    Some(commit_sha.to_string()),
                    Some(entity.clone()),
                    Some(family.clone()),
                    vec![delta.key.clone()],
                    format!(
                        "same-direction duplicate: `{}` applied twice in the stack",
                        delta.key
                    ),
                ));
            } else {
                chain.state += polarity;
                chain.last_polarity = polarity;
                chain.last_record = delta.clone();
                chain.last_commit = commit_sha.to_string();
            }
        }
    }
}

pub(super) fn apply_transition(
    state: &mut HashMap<(String, String), TransitionChain>,
    anomalies: &mut Vec<Anomaly>,
    family: String,
    entity: String,
    from: String,
    to: String,
    delta: &DeltaRecord,
    commit_sha: &str,
) {
    let key = (family.clone(), entity.clone());
    match state.get_mut(&key) {
        None => {
            state.insert(
                key,
                TransitionChain {
                    initial_from: from,
                    current_to: to,
                    last_record: delta.clone(),
                    last_commit: commit_sha.to_string(),
                    family,
                    entity,
                },
            );
        }
        Some(chain) => {
            if chain.current_to == from {
                chain.current_to = to;
                chain.last_record = delta.clone();
                chain.last_commit = commit_sha.to_string();
            } else {
                anomalies.push(build_anomaly(
                    AnomalyClass::AlgebraicChainBreak,
                    AnomalySeverity::Surface,
                    Some(commit_sha.to_string()),
                    Some(entity.clone()),
                    Some(family.clone()),
                    vec![delta.key.clone()],
                    format!(
                        "chain break on `{}`: expected `from = {}`, observed `from = {}`",
                        entity, chain.current_to, from
                    ),
                ));
            }
        }
    }
}
