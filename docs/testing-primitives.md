# Testing Primitives

Phase 26 makes testing part of the Corvid language, not an external convention.

## Test Declarations

`test name:` is a top-level declaration with setup statements followed by
assertions:

```corvid
tool get_order(id: String) -> String

test refund_contract:
    order = get_order("ord_42")
    assert called get_order
    assert order == "ord_42"
```

The compiler parses, resolves, typechecks, and lowers tests into IR. The runner
lands in the next slice, but test declarations already share the eval assertion
model:

- `assert <Bool expression>` checks ordinary values.
- `assert called tool_name` checks trace/process shape.
- `assert called A before B` checks ordering.
- `assert approved Label` checks approval process.
- `assert cost < $0.50` checks cost traces.
- `assert <Bool expression> with confidence P over N runs` is preserved for
  eval-compatible statistical assertions.

This is intentional. Tests and evals should not have competing assertion
languages. Tests are deterministic developer checks; evals add statistical LLM
behavior and model-quality reporting on top of the same compiler assertion
model.
