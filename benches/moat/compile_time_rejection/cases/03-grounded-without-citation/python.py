"""Python equivalent — passes mypy --strict. Citation forgery ships."""

from typing import Annotated
from pydantic import BaseModel


# A "Grounded" wrapper is convention-only in Python. Anyone can construct
# one without a real source; mypy has no way to track provenance flow.
class Grounded(BaseModel):
    value: str
    sources: list[str] = []


def fabricate(seed: str) -> str:
    return f"answer-for-{seed}"


# BUG: returns a Grounded with empty sources. The type checker is
# satisfied; the answer claims to be grounded but cites nothing.
def answer(seed: str) -> Grounded:
    return Grounded(value=fabricate(seed))
