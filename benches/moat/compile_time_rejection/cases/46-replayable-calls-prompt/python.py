"""Python — replayability is a docstring claim."""
def ask_human(q: str) -> str:
    # BUG: input() not captured by replay; mypy passes.
    return input(q)
