"""Python — stream confidence threshold convention-only."""
from pydantic import BaseModel


class StreamConfig(BaseModel):
    min_confidence: float


# BUG: out-of-range threshold accepted.
_cfg = StreamConfig(min_confidence=1.5)
