"""Python — citation parameter type not enforced."""
from pydantic import BaseModel


class Prompt(BaseModel):
    template: str
    cites_strictly: str


def summarise(question: str, ctx: str) -> str:
    return f"answer {question} using {ctx}"


# BUG: cites a non-grounded param; pydantic does not check.
_meta = Prompt(template="answer {question} using {ctx}", cites_strictly="question")
