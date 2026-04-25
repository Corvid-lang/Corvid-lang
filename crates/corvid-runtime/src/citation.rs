//! Runtime citation verification for `cites <ctx> strictly`.

/// Returns true when the response text cites a contiguous phrase from
/// the cited context. The heuristic is intentionally simple and local:
/// it is deterministic, offline, and identical across interpreter and
/// native execution.
pub fn citation_verified(context: &str, response: &str) -> bool {
    let ctx = context.to_ascii_lowercase();
    let resp = response.to_ascii_lowercase();
    if ctx.trim().is_empty() {
        return false;
    }

    let words: Vec<&str> = ctx.split_whitespace().collect();
    if words.len() < 4 {
        return resp.contains(ctx.trim());
    }

    words
        .windows(4)
        .map(|window| window.join(" "))
        .any(|phrase| resp.contains(&phrase))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn citation_verified_accepts_four_word_phrase() {
        assert!(citation_verified(
            "alpha beta gamma delta epsilon",
            "The answer cites beta gamma delta epsilon."
        ));
    }

    #[test]
    fn citation_verified_rejects_unrelated_response() {
        assert!(!citation_verified(
            "alpha beta gamma delta epsilon",
            "unrelated response"
        ));
    }
}
