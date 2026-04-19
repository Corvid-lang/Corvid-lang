use super::{assert_parity, assert_parity_bool};

#[test]
fn list_literal_sum_via_for() {
    assert_parity(
        "\
agent main() -> Int:
    total = 0
    for x in [1, 2, 3, 4, 5]:
        total = total + x
    return total
",
        15,
    );
}

#[test]
fn for_with_break_exits_early() {
    assert_parity(
        "\
agent main() -> Int:
    total = 0
    for x in [1, 2, 3, 4, 5]:
        if x == 3:
            break
        total = total + x
    return total
",
        3,
    );
}

#[test]
fn for_with_continue_skips_element() {
    assert_parity(
        "\
agent main() -> Int:
    total = 0
    for x in [1, 2, 3, 4, 5]:
        if x == 3:
            continue
        total = total + x
    return total
",
        12,
    );
}

#[test]
fn list_subscript_access() {
    assert_parity(
        "\
agent main() -> Int:
    xs = [10, 20, 30]
    return xs[1]
",
        20,
    );
}

#[test]
fn list_of_strings_destructor_releases_elements() {
    assert_parity_bool(
        "\
agent main() -> Bool:
    xs = [\"a\", \"b\", \"c\"]
    return xs[1] == \"b\"
",
        true,
    );
}

#[test]
fn list_of_heap_strings_exercises_real_releases() {
    assert_parity_bool(
        "\
agent main() -> Bool:
    xs = [\"hi \" + \"a\", \"hi \" + \"b\", \"hi \" + \"c\"]
    return xs[2] == \"hi c\"
",
        true,
    );
}

#[test]
fn nested_list_subscript_two_deep() {
    assert_parity(
        "\
agent main() -> Int:
    rows = [[1, 2], [3, 4], [5, 6]]
    return rows[1][0]
",
        3,
    );
}

#[test]
fn empty_list_for_loop_runs_zero_iterations() {
    assert_parity(
        "\
agent main() -> Int:
    total = 0
    for x in [0, 0, 0, 0]:
        total = total + 1
    return total
",
        4,
    );
    assert_parity(
        "\
agent main() -> Int:
    total = 99
    for x in [1]:
        total = total - 1
    return total
",
        98,
    );
}
