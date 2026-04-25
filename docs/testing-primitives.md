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

The compiler parses, resolves, typechecks, and lowers tests into IR. `corvid
test <file.cor>` discovers the lowered `IrTest` declarations, executes each
setup body in the interpreter, evaluates value assertions, and returns a CI
exit code:

```text
corvid test examples/math.cor
  PASS refund_contract

1 passed, 0 failed
```

The assertion language is shared with eval declarations:

- `assert <Bool expression>` checks ordinary values.
- `assert called tool_name` checks trace/process shape.
- `assert called A before B` checks ordering.
- `assert approved Label` checks approval process.
- `assert cost < $0.50` checks cost traces.
- `assert <Bool expression> with confidence P over N runs` is preserved for
  eval-compatible statistical assertions.

The Phase 26-B runner executes value assertions now, including statistical value
assertions by rerunning the test setup body for the requested number of runs.
Trace/process assertions are parsed, typechecked, and lowered, but `corvid test`
reports them as unsupported failures until Phase 26-E trace fixtures land. The
runner deliberately does not silently pass process assertions it cannot verify.

This is intentional. Tests and evals should not have competing assertion
languages. Tests are deterministic developer checks; evals add statistical LLM
behavior and model-quality reporting on top of the same compiler assertion
model.

## Fixtures

`fixture name(...) -> Type:` declares typed reusable test data:

```corvid
fixture order_id() -> String:
    return "ord_42"

test fixture_contract:
    id = order_id()
    assert id == "ord_42"
```

Fixtures are language declarations, not untyped runner macros. They parse,
resolve, typecheck, lower to `IrFixture`, and execute through the same
interpreter as test setup bodies. A fixture call is only callable from `test`
and `mock` bodies; calling a fixture from a production `agent` is rejected by
the typechecker.

The native, Python, and WASM codegen tiers treat fixture calls as
interpreter-only test infrastructure. They do not silently compile fixture
calls into production artifacts.

## Mocks

`mock tool_name(...) -> Type:` declares a typed test-only override for an
existing tool:

```corvid
tool lookup_score(id: String) -> Int

mock lookup_score(id: String) -> Int:
    if id == "ord_42":
        return 42
    return 0

test mocked_tool_contract:
    score = lookup_score("ord_42")
    assert score == 42
```

The mock signature must match the target tool exactly: arity, parameter types,
and return type. Mocks are active inside `corvid test` and are not registered as
normal callable declarations, so production code still calls the real tool
boundary.

Mocks preserve the target tool's effect profile. A mocked dangerous tool still
requires the same approval before it can be called. Runtime dispatch performs
the normal approval/confidence/tool-call gate first, then executes the mock body
instead of crossing the external host-tool boundary.

## Snapshot Assertions

`assert_snapshot <expr>` captures the evaluated value of an expression as a
deterministic JSON snapshot:

```corvid
test response_shape:
    value = "stable contract"
    assert_snapshot value
```

The first run writes the snapshot and reports the assertion as updated. Later
runs compare against the stored value and fail with a small unified diff if the
value changed.

Snapshots are stored next to the source file under a deterministic path:

```text
.corvid-snapshots/<source-stem>/<test-name>__001.snap
```

Multiple snapshot assertions in one test use `__002.snap`, `__003.snap`, and so
on. Path segments are sanitized so snapshot filenames are stable across shells
and platforms.

To intentionally accept changed output, run:

```text
corvid test --update-snapshots path/to/file.cor
```

The driver also honors `CORVID_UPDATE_SNAPSHOTS=1` for CI jobs that want an
explicit update mode. Update mode rewrites existing snapshots and creates
missing ones; normal mode only creates missing first-run snapshots and fails on
mismatches.

Snapshot assertions are value assertions over runtime data, not text rewrites.
The VM evaluates the expression through the same interpreter path as ordinary
tests, converts the value to JSON, and compares that JSON to the stored
snapshot. This means typed fixtures and mocks compose with snapshots without a
separate snapshot DSL.
