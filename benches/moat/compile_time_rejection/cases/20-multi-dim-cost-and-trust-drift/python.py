"""Python — multi-dim composition is runtime."""
def settle() -> None:
    return None


def bad() -> None:
    # BUG: cumulative cost + trust both drift past the caller's intent.
    return settle()
