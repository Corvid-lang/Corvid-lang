"""Python — helper-composed cost is runtime-tracked."""
def burner(x: str) -> str:
    return x


def helper(x: str) -> str:
    return burner(x)


def caller(x: str, budget_usd: float = 0.10) -> str:
    # BUG: helper cost > caller budget; mypy passes.
    return helper(x)
