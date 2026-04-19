//! Snake-case <-> PascalCase conversion helpers.
//!
//! Used by the approve-label matcher (`check_tool_call` in call.rs)
//! to derive the expected PascalCase label for a snake_case tool
//! name. Also by the diagnostic path that spells the required label
//! back at the user.
//!
//! Extracted from `checker.rs` as part of Phase 20i responsibility
//! decomposition.

pub(super) fn pascal_case(snake: &str) -> String {
    let mut out = String::new();
    let mut cap_next = true;
    for c in snake.chars() {
        if c == '_' {
            cap_next = true;
            continue;
        }
        if cap_next {
            out.extend(c.to_uppercase());
            cap_next = false;
        } else {
            out.push(c);
        }
    }
    out
}

pub(super) fn snake_case(pascal: &str) -> String {
    let mut out = String::new();
    for (i, c) in pascal.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snake_and_pascal_are_inverses() {
        assert_eq!(pascal_case("issue_refund"), "IssueRefund");
        assert_eq!(snake_case("IssueRefund"), "issue_refund");
        assert_eq!(pascal_case("send_email"), "SendEmail");
        assert_eq!(snake_case("SendEmail"), "send_email");
    }
}
