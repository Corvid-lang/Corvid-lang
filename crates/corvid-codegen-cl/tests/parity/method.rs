use super::{assert_parity, assert_parity_bool};

#[test]
fn method_returns_field() {
    assert_parity(
        "type Amount:\n    cents: Int\n\nextend Amount:\n    public agent value(a: Amount) -> Int:\n        return a.cents\n\nagent main() -> Int:\n    a = Amount(42)\n    return a.value()\n",
        42,
    );
}

#[test]
fn method_with_arithmetic_on_field() {
    assert_parity(
        "type Order:\n    amount: Int\n    tax: Int\n\nextend Order:\n    public agent total(o: Order) -> Int:\n        return o.amount + o.tax\n\nagent main() -> Int:\n    o = Order(100, 7)\n    return o.total()\n",
        107,
    );
}

#[test]
fn method_with_extra_arg_after_receiver() {
    assert_parity(
        "type Amount:\n    cents: Int\n\nextend Amount:\n    public agent scale(a: Amount, factor: Int) -> Int:\n        return a.cents * factor\n\nagent main() -> Int:\n    a = Amount(7)\n    return a.scale(6)\n",
        42,
    );
}

#[test]
fn method_calls_another_method() {
    assert_parity(
        "type Bill:\n    base: Int\n\nextend Bill:\n    public agent with_tip(b: Bill, pct: Int) -> Int:\n        return b.base + (b.base * pct) / 100\n    public agent total(b: Bill) -> Int:\n        return b.with_tip(20)\n\nagent main() -> Int:\n    b = Bill(100)\n    return b.total()\n",
        120,
    );
}

#[test]
fn methods_with_same_name_on_different_types() {
    assert_parity(
        "type Order:\n    amount: Int\n\ntype Line:\n    units: Int\n\nextend Order:\n    public agent total(o: Order) -> Int:\n        return o.amount\n\nextend Line:\n    public agent total(l: Line) -> Int:\n        return l.units * 10\n\nagent main() -> Int:\n    o = Order(5)\n    l = Line(3)\n    return o.total() + l.total()\n",
        35,
    );
}

#[test]
fn method_with_string_field_releases_correctly() {
    assert_parity_bool(
        "type Named:\n    label: String\n\nextend Named:\n    public agent matches(n: Named, query: String) -> Bool:\n        return n.label == query\n\nagent main() -> Bool:\n    n = Named(\"hello\")\n    return n.matches(\"hello\")\n",
        true,
    );
}
