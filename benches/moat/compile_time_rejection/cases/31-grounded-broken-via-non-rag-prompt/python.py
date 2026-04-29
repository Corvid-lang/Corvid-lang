"""Python — prompt result wrapped in Grounded with empty sources."""
from pydantic import BaseModel


class Grounded(BaseModel):
    value: str
    sources: list[str] = []


def opinion(q: str) -> str:
    return f"my view on {q}"


def answer(q: str) -> Grounded:
    # BUG: opinion has no provenance; sources empty.
    return Grounded(value=opinion(q))
