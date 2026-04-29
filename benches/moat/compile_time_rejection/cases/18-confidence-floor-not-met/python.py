"""Python — confidence dimension is runtime metadata."""
from pydantic import BaseModel


class Confidence(BaseModel):
    score: float


def shaky_lookup(q: str) -> str:
    return q


def answer(q: str, threshold: float = 0.95) -> str:
    # BUG: tool's known confidence (0.70) below caller's threshold; mypy passes.
    return shaky_lookup(q)
