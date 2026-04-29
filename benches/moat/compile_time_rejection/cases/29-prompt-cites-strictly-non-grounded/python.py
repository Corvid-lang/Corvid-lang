"""Python — prompt template is a string; citation contract is convention."""
from pydantic import BaseModel


class Prompt(BaseModel):
    template: str
    cites_strictly: str


def summarise(ctx: str) -> str:
    return f"summarise: {ctx}"


_metadata = Prompt(template="summarise: {ctx}", cites_strictly="ctx")
