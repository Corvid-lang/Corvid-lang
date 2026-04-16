//! Compile-time + run-time verification that `#[tool]` produces the
//! contract this macro layer promises:
//!
//!   1. The user's `async fn` remains callable as plain Rust.
//!   2. A `#[no_mangle] pub extern "C" fn __corvid_tool_<name>` wrapper
//!      exists with the expected typed-ABI signature.
//!   3. A `ToolMetadata` entry is visible via `inventory::iter`.
//!
//! End-to-end invocation of the wrapper (which would need the runtime
//! bridge + tokio + the C runtime linked) is handled elsewhere — this
//! test only verifies the macro contract, not dispatch.

use corvid_runtime::{abi::ToolMetadata, inventory};

// Bring the proc-macro into scope with its canonical name.
use corvid_macros::tool;

// ---- (1) Declarations used by the tests below ----

/// Simple scalar round-trip: Int in, Int out. The most common tool shape.
#[tool("double_it")]
async fn double_it(n: i64) -> i64 {
    n * 2
}

/// Boolean flip. Verifies the `bool` ABI path compiles.
#[tool("flip")]
async fn flip(b: bool) -> bool {
    !b
}

/// Float input, Float output. Verifies the `f64` ABI path compiles.
#[tool("round_trip_float")]
async fn round_trip_float(x: f64) -> f64 {
    x + 1.0
}

/// Zero-arg tool returning Int — same shape as the narrow
/// bridge used to support directly. Preserves that capability.
#[tool("zero_arg_answer")]
async fn zero_arg_answer() -> i64 {
    42
}

// ---- (2) The user's async fns remain callable as plain Rust ----

#[tokio::test]
async fn user_async_fn_still_callable_directly() {
    assert_eq!(double_it(7).await, 14);
    assert!(!flip(true).await);
    assert_eq!(round_trip_float(1.5).await, 2.5);
    assert_eq!(zero_arg_answer().await, 42);
}

// ---- (3) `inventory` sees every `#[tool]` metadata entry ----

#[test]
fn inventory_collects_every_tool() {
    let names: Vec<&'static str> = inventory::iter::<ToolMetadata>()
        .into_iter()
        .map(|m| m.name)
        .collect();

    for expected in ["double_it", "flip", "round_trip_float", "zero_arg_answer"] {
        assert!(
            names.contains(&expected),
            "inventory missing `{expected}`; saw {names:?}"
        );
    }
}

#[test]
fn metadata_arity_matches_declared_signature() {
    let by_name: std::collections::HashMap<&'static str, &'static ToolMetadata> =
        inventory::iter::<ToolMetadata>()
            .into_iter()
            .map(|m| (m.name, m))
            .collect();

    assert_eq!(by_name.get("double_it").unwrap().arity, 1);
    assert_eq!(by_name.get("flip").unwrap().arity, 1);
    assert_eq!(by_name.get("round_trip_float").unwrap().arity, 1);
    assert_eq!(by_name.get("zero_arg_answer").unwrap().arity, 0);
}

#[test]
fn metadata_symbol_follows_convention() {
    let by_name: std::collections::HashMap<&'static str, &'static ToolMetadata> =
        inventory::iter::<ToolMetadata>()
            .into_iter()
            .map(|m| (m.name, m))
            .collect();

    // The symbol is what Cranelift codegen will emit a direct call to —
    // stability across refactors matters. Locked convention here so a
    // future macro change that breaks it gets caught.
    assert_eq!(
        by_name.get("double_it").unwrap().symbol,
        "__corvid_tool_double_it"
    );
    assert_eq!(
        by_name.get("zero_arg_answer").unwrap().symbol,
        "__corvid_tool_zero_arg_answer"
    );
}
