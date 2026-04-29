"""Python — purity is convention, not type."""
def external(x: str) -> str:
    return x


def compute(x: str) -> str:
    # BUG: nothing in mypy enforces purity.
    return external(x)
