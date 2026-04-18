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
}
