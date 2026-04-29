"""Python — input() is convention, no purity check."""
def answer(q: str) -> str:
    # BUG: input() is non-deterministic; mypy passes.
    return input(q)
