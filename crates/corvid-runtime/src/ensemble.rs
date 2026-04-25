use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub struct EnsembleVoteOutcome {
    pub winner: String,
    pub agreement_rate: f64,
}

pub fn majority_vote(results: &[String]) -> EnsembleVoteOutcome {
    debug_assert!(
        !results.is_empty(),
        "majority_vote requires at least one result"
    );

    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for result in results {
        *counts.entry(result.clone()).or_default() += 1;
    }

    let (winner, winner_count) = counts
        .into_iter()
        .max_by(|(left_text, left_count), (right_text, right_count)| {
            left_count
                .cmp(right_count)
                .then_with(|| right_text.cmp(left_text))
        })
        .expect("counts cannot be empty");

    EnsembleVoteOutcome {
        agreement_rate: winner_count as f64 / results.len() as f64,
        winner,
    }
}

pub fn weighted_vote(results: &[String], weights: &[f64]) -> EnsembleVoteOutcome {
    debug_assert!(
        !results.is_empty(),
        "weighted_vote requires at least one result"
    );
    debug_assert_eq!(
        results.len(),
        weights.len(),
        "weighted_vote requires one weight per result"
    );

    let mut counts: BTreeMap<String, f64> = BTreeMap::new();
    let mut total_weight = 0.0_f64;
    for (result, weight) in results.iter().zip(weights.iter()) {
        let weight = weight.max(0.0);
        total_weight += weight;
        *counts.entry(result.clone()).or_default() += weight;
    }

    if total_weight <= f64::EPSILON {
        return majority_vote(results);
    }

    let (winner, winner_weight) = counts
        .into_iter()
        .max_by(|(left_text, left_count), (right_text, right_count)| {
            left_count
                .partial_cmp(right_count)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| right_text.cmp(left_text))
        })
        .expect("counts cannot be empty");

    EnsembleVoteOutcome {
        agreement_rate: winner_weight / total_weight,
        winner,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn majority_vote_picks_plurality() {
        let outcome = majority_vote(&[
            "alpha".to_string(),
            "beta".to_string(),
            "alpha".to_string(),
        ]);
        assert_eq!(
            outcome,
            EnsembleVoteOutcome {
                winner: "alpha".to_string(),
                agreement_rate: 2.0 / 3.0,
            }
        );
    }

    #[test]
    fn majority_vote_breaks_ties_alphabetically() {
        let outcome = majority_vote(&[
            "zulu".to_string(),
            "alpha".to_string(),
        ]);
        assert_eq!(
            outcome,
            EnsembleVoteOutcome {
                winner: "alpha".to_string(),
                agreement_rate: 0.5,
            }
        );
    }

    #[test]
    fn weighted_vote_uses_weights_and_breaks_ties_alphabetically() {
        let outcome = weighted_vote(
            &["alpha".to_string(), "beta".to_string(), "beta".to_string()],
            &[0.9, 0.2, 0.2],
        );
        assert_eq!(
            outcome,
            EnsembleVoteOutcome {
                winner: "alpha".to_string(),
                agreement_rate: 0.9 / 1.3,
            }
        );

        let tie = weighted_vote(&["zulu".to_string(), "alpha".to_string()], &[1.0, 1.0]);
        assert_eq!(tie.winner, "alpha");
    }
}
