"""Python — provenance is metadata, not flow."""
from pydantic import BaseModel


class Grounded(BaseModel):
    value: str
    sources: list[str] = []


def search(q: str) -> str:
    return f"hit for {q}"


def strip_provenance(q: str) -> str:
    return search(q)


def answer(q: str) -> Grounded:
    # BUG: provenance dropped at the helper boundary; mypy passes.
    return Grounded(value=strip_provenance(q))
