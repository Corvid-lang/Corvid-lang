use super::assert_parity_bool;

#[test]
fn float_addition_eq_check() {
    assert_parity_bool(
        "agent f() -> Bool:\n    return 1.5 + 2.5 == 4.0\n",
        true,
    );
}

#[test]
fn float_subtraction_and_multiplication() {
    assert_parity_bool(
        "agent f() -> Bool:\n    return (5.0 - 1.5) * 2.0 == 7.0\n",
        true,
    );
}

#[test]
fn float_division_exact() {
    assert_parity_bool(
        "agent f() -> Bool:\n    return 9.0 / 3.0 == 3.0\n",
        true,
    );
}

#[test]
fn mixed_int_float_promotes_to_float() {
    assert_parity_bool(
        "agent f() -> Bool:\n    return 3 + 0.5 == 3.5\n",
        true,
    );
    assert_parity_bool(
        "agent f() -> Bool:\n    return 0.5 + 3 == 3.5\n",
        true,
    );
}

#[test]
fn float_ordering() {
    assert_parity_bool("agent f() -> Bool:\n    return 1.5 < 2.0\n", true);
    assert_parity_bool("agent f() -> Bool:\n    return 2.0 <= 2.0\n", true);
    assert_parity_bool("agent f() -> Bool:\n    return 3.14 > 3.0\n", true);
    assert_parity_bool("agent f() -> Bool:\n    return 3.0 >= 3.0\n", true);
}

#[test]
fn float_unary_negation() {
    assert_parity_bool(
        "agent f() -> Bool:\n    return -2.5 == 0.0 - 2.5\n",
        true,
    );
}

#[test]
fn float_div_by_zero_is_infinity_not_trap() {
    assert_parity_bool(
        "agent f() -> Bool:\n    return 1.0 / 0.0 > 1.0\n",
        true,
    );
}

#[test]
fn nan_inequality_is_true() {
    assert_parity_bool(
        "agent f() -> Bool:\n    return (0.0 / 0.0) != (0.0 / 0.0)\n",
        true,
    );
    assert_parity_bool(
        "agent f() -> Bool:\n    return (0.0 / 0.0) == (0.0 / 0.0)\n",
        false,
    );
}

#[test]
fn float_in_local_binding() {
    assert_parity_bool(
        "\
agent f() -> Bool:
    pi = 3.14
    tau = pi * 2.0
    return tau > 6.0
",
        true,
    );
}
