"""Python — token budgets tracked at runtime, if at all."""
def talkative(q: str) -> str:
    return q


def quiet(q: str, budget_tokens: int = 2000) -> str:
    # BUG: known token cost > intended budget; mypy passes.
    return talkative(q)
