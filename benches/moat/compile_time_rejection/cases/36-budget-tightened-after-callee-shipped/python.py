"""Python — budget annotation is convention; no compile-time recheck."""
def work(x: str) -> str:
    return x


def op(x: str, budget_usd: float = 0.20) -> str:
    # BUG: tighter budget passes mypy; runtime overrun ships.
    return work(x)
