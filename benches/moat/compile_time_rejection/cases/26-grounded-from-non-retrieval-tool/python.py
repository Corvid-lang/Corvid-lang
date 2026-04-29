"""Python — provenance is convention, not type."""
from pydantic import BaseModel


class Grounded(BaseModel):
    value: str
    sources: list[str] = []


def ask_model(q: str) -> str:
    return f"likely answer for {q}"


def answer(q: str) -> Grounded:
    # BUG: ask_model isn't a retrieval tool; sources empty.
    return Grounded(value=ask_model(q))
