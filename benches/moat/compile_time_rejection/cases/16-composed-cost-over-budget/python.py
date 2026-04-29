"""Python — cost tracking is runtime, not compile-time."""
from pydantic import BaseModel


class Cost(BaseModel):
    usd: float


def classify(text: str) -> str:
    return text


def summarise(text: str) -> str:
    return text


def pipeline(text: str) -> str:
    # BUG: cumulative cost ($0.60) exceeds intended ceiling ($0.50).
    # mypy / pydantic have nothing to say about this.
    return summarise(classify(text))
