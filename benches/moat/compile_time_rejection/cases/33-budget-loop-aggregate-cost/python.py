"""Python — aggregate cost runtime-tracked, not budget-bound."""
def ask(q: str) -> str:
    return q


def process(q: str, budget_usd: float = 0.05) -> str:
    # BUG: aggregate cost > budget; mypy passes.
    a = ask(q)
    b = ask(q)
    return a + b
