use super::assert_parity_bool;

#[test]
fn string_literal_equality_is_true() {
    assert_parity_bool(
        "agent f() -> Bool:\n    return \"hello\" == \"hello\"\n",
        true,
    );
}

#[test]
fn string_literal_inequality_is_false() {
    assert_parity_bool(
        "agent f() -> Bool:\n    return \"hello\" == \"world\"\n",
        false,
    );
}

#[test]
fn string_concat_then_compare() {
    assert_parity_bool(
        "agent f() -> Bool:\n    return \"hi \" + \"there\" == \"hi there\"\n",
        true,
    );
}

#[test]
fn empty_string_concat_is_identity() {
    assert_parity_bool(
        "agent f() -> Bool:\n    return \"\" + \"x\" == \"x\"\n",
        true,
    );
    assert_parity_bool(
        "agent f() -> Bool:\n    return \"x\" + \"\" == \"x\"\n",
        true,
    );
}

#[test]
fn string_not_equal_operator() {
    assert_parity_bool(
        "agent f() -> Bool:\n    return \"hello\" != \"world\"\n",
        true,
    );
    assert_parity_bool(
        "agent f() -> Bool:\n    return \"hello\" != \"hello\"\n",
        false,
    );
}

#[test]
fn string_ordering_lexicographic() {
    assert_parity_bool(
        "agent f() -> Bool:\n    return \"abc\" < \"abd\"\n",
        true,
    );
    assert_parity_bool(
        "agent f() -> Bool:\n    return \"abc\" <= \"abc\"\n",
        true,
    );
    assert_parity_bool(
        "agent f() -> Bool:\n    return \"abd\" > \"abc\"\n",
        true,
    );
    assert_parity_bool(
        "agent f() -> Bool:\n    return \"abc\" >= \"abc\"\n",
        true,
    );
}

#[test]
fn string_in_local_binding_then_concat_then_compare() {
    assert_parity_bool(
        "\
agent f() -> Bool:
    s = \"foo\"
    s = s + \"bar\"
    return s == \"foobar\"
",
        true,
    );
}
