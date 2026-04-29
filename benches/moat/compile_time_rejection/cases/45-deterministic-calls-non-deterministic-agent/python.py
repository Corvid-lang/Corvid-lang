"""Python — purity contagion is not modelled."""
def helper(x: str) -> str:
    return x


def compute(x: str) -> str:
    # BUG: helper is not annotated pure; mypy passes.
    return helper(x)
