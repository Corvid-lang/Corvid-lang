use super::{assert_parity, assert_parity_bool, ir_of, test_tools_lib_path};

#[test]
fn struct_entry_return_is_blocked_with_clear_error() {
    use corvid_codegen_cl::{build_native_to_disk, CodegenErrorKind};

    let ir = ir_of(
        "type Wrap:\n    v: Int\n\nagent f() -> Wrap:\n    return Wrap(42)\n",
    );
    let tmp = tempfile::tempdir().unwrap();
    let bin_path = tmp.path().join("prog");
    let err = build_native_to_disk(
        &ir,
        "corvid_parity_test",
        &bin_path,
        &[test_tools_lib_path().as_path()],
    )
    .unwrap_err();
    match err.kind {
        CodegenErrorKind::NotSupported(ref msg) => {
            assert!(
                msg.contains("struct") || msg.contains("Struct") || msg.contains("Wrap"),
                "expected message to mention struct: {msg}"
            );
            assert!(
                msg.contains("serialization"),
                "expected message to point at missing serialization support: {msg}"
            );
        }
        other => panic!("expected NotSupported, got {other:?}"),
    }
}

#[test]
fn scalar_only_struct_construct_and_access() {
    assert_parity(
        "\
type Point:
    x: Int
    y: Int

agent main() -> Int:
    p = Point(3, 4)
    return p.x + p.y
",
        7,
    );
}

#[test]
fn struct_with_bool_field() {
    assert_parity_bool(
        "\
type Flag:
    enabled: Bool
    code: Int

agent main() -> Bool:
    f = Flag(true, 42)
    return f.enabled
",
        true,
    );
}

#[test]
fn struct_with_string_field_destructor_releases_field() {
    assert_parity_bool(
        "\
type Order:
    id: String
    amount: Float

agent main() -> Bool:
    o = Order(\"ord_1\", 49.99)
    return o.amount > 10.0
",
        true,
    );
}

#[test]
fn struct_with_string_field_extract_and_compare() {
    assert_parity_bool(
        "\
type Named:
    label: String

agent main() -> Bool:
    n = Named(\"hello\")
    return n.label == \"hello\"
",
        true,
    );
}

#[test]
fn struct_passed_to_another_agent() {
    assert_parity(
        "\
type Amount:
    cents: Int

agent total(a: Amount, b: Amount) -> Int:
    return a.cents + b.cents

agent main() -> Int:
    x = Amount(100)
    y = Amount(250)
    return total(x, y)
",
        350,
    );
}

#[test]
fn struct_reassignment_releases_old_instance() {
    assert_parity(
        "\
type Box:
    v: Int

agent main() -> Int:
    b = Box(1)
    b = Box(100)
    return b.v
",
        100,
    );
}

#[test]
fn nested_struct_field_access() {
    assert_parity(
        "\
type Inner:
    value: Int

type Outer:
    inner: Inner
    tag: Int

agent main() -> Int:
    i = Inner(7)
    o = Outer(i, 10)
    return o.inner.value + o.tag
",
        17,
    );
}
