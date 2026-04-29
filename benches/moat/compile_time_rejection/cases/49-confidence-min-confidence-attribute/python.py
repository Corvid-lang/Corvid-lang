"""Python — effect-confidence is convention metadata."""
from pydantic import BaseModel


class EffectMeta(BaseModel):
    name: str
    confidence: float


# BUG: out-of-range confidence accepted by pydantic.
_meta = EffectMeta(name="bad_conf", confidence=1.7)
