"""Python — latency tracking is runtime."""
def slow_lookup(q: str) -> str:
    return q


def fast_path(q: str, budget_ms: int = 500) -> str:
    # BUG: known latency > intended budget; mypy passes.
    return slow_lookup(q)
