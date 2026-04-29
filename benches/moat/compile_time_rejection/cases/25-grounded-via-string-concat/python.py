"""Python — wrapper accepts hand-built strings without sources."""
from pydantic import BaseModel


class Grounded(BaseModel):
    value: str
    sources: list[str] = []


def answer(seed: str) -> Grounded:
    # BUG: built by string concat; no source.
    return Grounded(value="answer-for-" + seed)
