"""Python — trust on a prompt is convention-only."""
from typing import Literal
from pydantic import BaseModel


TrustLevel = Literal["autonomous", "human_required"]


class Prompt(BaseModel):
    template: str
    trust: TrustLevel


def advise(q: str) -> str:
    return f"advice for {q}"


def ask(q: str, declared_trust: TrustLevel = "autonomous") -> str:
    # BUG: advise's trust dimension wider than declared; mypy passes.
    return advise(q)
