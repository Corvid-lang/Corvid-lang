"""Python — progressive threshold convention-only."""
from pydantic import BaseModel


class StageConfig(BaseModel):
    model: str
    threshold: float


# BUG: out-of-range threshold accepted.
_stages = [StageConfig(model="fast", threshold=1.2), StageConfig(model="strong", threshold=0.0)]
