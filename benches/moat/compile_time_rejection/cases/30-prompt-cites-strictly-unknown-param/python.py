"""Python — citation parameter name is a free-form string."""
from pydantic import BaseModel


class Prompt(BaseModel):
    template: str
    cites_strictly: str


def summarise(ctx: str) -> str:
    return f"summarise: {ctx}"


_metadata = Prompt(template="summarise: {ctx}", cites_strictly="context")
