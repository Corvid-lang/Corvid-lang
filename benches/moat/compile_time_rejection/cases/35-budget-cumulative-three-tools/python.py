"""Python — three tools, runtime cost tracking."""
def a(x: str) -> str:
    return x


def b(x: str) -> str:
    return x


def c(x: str) -> str:
    return x


def pipeline(x: str, budget_usd: float = 0.50) -> str:
    # BUG: cumulative cost > budget.
    return c(b(a(x)))
