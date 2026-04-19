use super::assert_parity_bool;

#[test]
fn weak_upgrade_is_live_while_strong_value_is_still_in_scope() {
    assert_parity_bool(
        "agent main() -> Bool:\n    s = \"hello\"\n    w = Weak::new(s)\n    return Weak::upgrade(w) != None\n",
        true,
    );
}
