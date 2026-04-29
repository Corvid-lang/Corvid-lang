"""Python — purity contagion not modelled."""
def inner(x: str) -> str:
    return x


def middle(x: str) -> str:
    return input(x)


def outer(x: str) -> str:
    # BUG: outer transitively non-deterministic; mypy passes.
    return middle(inner(x))
