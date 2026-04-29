"""Python — prompts are functions; mypy passes."""
def classify(x: str) -> str:
    return f"label-for-{x}"


def compute(x: str) -> str:
    # BUG: prompt call non-deterministic; mypy passes.
    return classify(x)
