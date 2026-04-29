"""Python — budget tracking is runtime."""
def burner(x: str) -> str:
    return x


def over(x: str, budget_usd: float = 0.05) -> str:
    # BUG: known cost > intended budget; mypy passes.
    return burner(x)
