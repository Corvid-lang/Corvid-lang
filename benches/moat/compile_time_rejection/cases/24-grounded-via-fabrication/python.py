"""Python — Grounded wrapper accepts an empty source list."""
from pydantic import BaseModel


class Grounded(BaseModel):
    value: str
    sources: list[str] = []


def fabricate(seed: str) -> str:
    return f"answer-{seed}"


def helper(seed: str) -> str:
    return fabricate(seed)


def answer(seed: str) -> Grounded:
    # BUG: empty sources; mypy passes.
    return Grounded(value=helper(seed))
