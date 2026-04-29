"""Python — Grounded wraps any string."""
from pydantic import BaseModel


class Grounded(BaseModel):
    value: str
    sources: list[str] = []


def answer(q: str) -> Grounded:
    # BUG: literal value with no sources.
    return Grounded(value="the sky is blue")
