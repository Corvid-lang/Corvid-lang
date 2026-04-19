use super::{assert_parity, assert_parity_bool, assert_parity_overflow};

#[test]
fn bool_literal_true() {
    assert_parity_bool("agent t() -> Bool:\n    return true\n", true);
}

#[test]
fn bool_literal_false() {
    assert_parity_bool("agent f() -> Bool:\n    return false\n", false);
}

#[test]
fn int_equality() {
    assert_parity_bool("agent e() -> Bool:\n    return 3 == 3\n", true);
    assert_parity_bool("agent e() -> Bool:\n    return 3 == 4\n", false);
}

#[test]
fn int_inequality() {
    assert_parity_bool("agent n() -> Bool:\n    return 3 != 4\n", true);
    assert_parity_bool("agent n() -> Bool:\n    return 3 != 3\n", false);
}

#[test]
fn int_ordering() {
    assert_parity_bool("agent lt() -> Bool:\n    return 1 < 2\n", true);
    assert_parity_bool("agent lte() -> Bool:\n    return 2 <= 2\n", true);
    assert_parity_bool("agent gt() -> Bool:\n    return 2 > 1\n", true);
    assert_parity_bool("agent gte() -> Bool:\n    return 2 >= 2\n", true);
}

#[test]
fn not_flips_bool() {
    assert_parity_bool("agent n() -> Bool:\n    return not true\n", false);
    assert_parity_bool("agent n() -> Bool:\n    return not false\n", true);
}

#[test]
fn unary_negation_on_int() {
    assert_parity("agent n() -> Int:\n    return -5\n", -5);
    assert_parity("agent n() -> Int:\n    return -(2 + 3)\n", -5);
}

#[test]
fn unary_negation_of_int_min_is_parity_error() {
    assert_parity_overflow(
        "agent oops() -> Int:\n    return -(0 - 9223372036854775807 - 1)\n",
    );
}

#[test]
fn if_then_else_picks_then_branch() {
    assert_parity(
        "\
agent pick() -> Int:
    if 1 < 2:
        return 10
    else:
        return 20
",
        10,
    );
}

#[test]
fn if_then_else_picks_else_branch() {
    assert_parity(
        "\
agent pick() -> Int:
    if 2 < 1:
        return 10
    else:
        return 20
",
        20,
    );
}

#[test]
fn if_without_else_falls_through_on_false() {
    assert_parity(
        "\
agent run() -> Int:
    if 2 < 1:
        return 10
    return 99
",
        99,
    );
}

#[test]
fn if_without_else_takes_then_on_true() {
    assert_parity(
        "\
agent run() -> Int:
    if 1 < 2:
        return 10
    return 99
",
        10,
    );
}

#[test]
fn nested_if_else() {
    assert_parity(
        "\
agent run() -> Int:
    if 1 < 2:
        if 3 < 4:
            return 1
        else:
            return 2
    else:
        return 3
",
        1,
    );
}

#[test]
fn short_circuit_and_evaluates_rhs_only_when_lhs_true() {
    assert_parity_bool(
        "agent sc() -> Bool:\n    return true and (1 == 1)\n",
        true,
    );
    assert_parity_bool(
        "agent sc() -> Bool:\n    return false and (1 == 1)\n",
        false,
    );
}

#[test]
fn short_circuit_or_evaluates_rhs_only_when_lhs_false() {
    assert_parity_bool(
        "agent sc() -> Bool:\n    return true or (1 == 1)\n",
        true,
    );
    assert_parity_bool(
        "agent sc() -> Bool:\n    return false or (1 == 1)\n",
        true,
    );
}

#[test]
fn short_circuit_or_skips_div_by_zero_on_rhs() {
    assert_parity_bool(
        "agent sc() -> Bool:\n    return true or (1 / (3 - 3) == 0)\n",
        true,
    );
}

#[test]
fn short_circuit_and_skips_div_by_zero_on_rhs() {
    assert_parity_bool(
        "agent sc() -> Bool:\n    return false and (1 / (3 - 3) == 0)\n",
        false,
    );
}

#[test]
fn bool_returning_agent_is_even() {
    assert_parity_bool(
        "agent is_even() -> Bool:\n    return 4 % 2 == 0\n",
        true,
    );
}

#[test]
fn local_binding_returns_value() {
    assert_parity("agent run() -> Int:\n    x = 42\n    return x\n", 42);
}

#[test]
fn local_binding_with_arithmetic() {
    assert_parity(
        "\
agent run() -> Int:
    x = 2
    y = 3
    return x + y * 4
",
        14,
    );
}

#[test]
fn local_binding_used_twice() {
    assert_parity(
        "\
agent run() -> Int:
    x = 7
    return x + x
",
        14,
    );
}

#[test]
fn reassignment_takes_latest_value() {
    assert_parity(
        "\
agent run() -> Int:
    x = 5
    x = x * 2
    x = x + 1
    return x
",
        11,
    );
}

#[test]
fn local_binding_with_bool() {
    assert_parity_bool(
        "\
agent run() -> Bool:
    flag = true
    return flag
",
        true,
    );
}

#[test]
fn reassignment_inside_if_branch() {
    assert_parity(
        "\
agent run() -> Int:
    x = 5
    if x == 5:
        x = 100
    return x
",
        100,
    );
}

#[test]
fn local_binding_used_in_comparison() {
    assert_parity_bool(
        "\
agent run() -> Bool:
    n = 4
    return n % 2 == 0
",
        true,
    );
}

#[test]
fn pass_in_if_is_a_noop() {
    assert_parity(
        "\
agent run() -> Int:
    x = 5
    if x > 0:
        pass
    return x
",
        5,
    );
}
