use super::{assert_parity, assert_parity_overflow};

#[test]
fn literal_return() {
    assert_parity("agent answer() -> Int:\n    return 42\n", 42);
}

#[test]
fn literal_negative() {
    assert_parity("agent answer() -> Int:\n    return 0 - 7\n", -7);
}

#[test]
fn add_two_literals() {
    assert_parity("agent calc() -> Int:\n    return 2 + 3\n", 5);
}

#[test]
fn subtract_two_literals() {
    assert_parity("agent calc() -> Int:\n    return 10 - 4\n", 6);
}

#[test]
fn multiply_two_literals() {
    assert_parity("agent calc() -> Int:\n    return 6 * 7\n", 42);
}

#[test]
fn divide_two_literals() {
    assert_parity("agent calc() -> Int:\n    return 20 / 4\n", 5);
}

#[test]
fn modulo_two_literals() {
    assert_parity("agent calc() -> Int:\n    return 23 % 5\n", 3);
}

#[test]
fn precedence_add_mul() {
    assert_parity("agent calc() -> Int:\n    return 1 + 2 * 3\n", 7);
}

#[test]
fn precedence_mul_add() {
    assert_parity("agent calc() -> Int:\n    return 2 * 3 + 1\n", 7);
}

#[test]
fn nested_arithmetic_long() {
    assert_parity(
        "agent calc() -> Int:\n    return 100 - 3 * 7 + 2\n",
        100 - 3 * 7 + 2,
    );
}

#[test]
fn recursive_agent_to_agent_call() {
    assert_parity(
        "\
agent helper() -> Int:
    return 41

agent main() -> Int:
    return helper() + 1
",
        42,
    );
}

#[test]
fn chained_agent_calls() {
    assert_parity(
        "\
agent a() -> Int:
    return 2

agent b() -> Int:
    return a() * 3

agent main() -> Int:
    return b() + 1
",
        7,
    );
}

#[test]
fn overflow_on_add_is_parity_error() {
    assert_parity_overflow(
        "agent oops() -> Int:\n    return 9223372036854775807 + 1\n",
    );
}

#[test]
fn wrapping_attribute_wraps_add_overflow() {
    assert_parity(
        "\
@wrapping
agent calc() -> Int:
    return 9223372036854775807 + 1
",
        i64::MIN,
    );
}

#[test]
fn wrapping_attribute_wraps_unary_negation() {
    assert_parity(
        "\
@wrapping
agent calc() -> Int:
    return -(-9223372036854775807 - 1)
",
        i64::MIN,
    );
}

#[test]
fn division_by_zero_is_parity_error() {
    assert_parity_overflow(
        "agent oops() -> Int:\n    return 10 / (3 - 3)\n",
    );
}

#[test]
fn modulo_by_zero_is_parity_error() {
    assert_parity_overflow(
        "agent oops() -> Int:\n    return 10 % (3 - 3)\n",
    );
}
