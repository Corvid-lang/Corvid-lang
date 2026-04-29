"""Python — composed cost tracking is runtime."""
def burner(x: str) -> str:
    return x


def helper(x: str) -> str:
    return burner(x)


def outer(x: str) -> str:
    # BUG: helper's cumulative cost > intended outer budget.
    return helper(x)
