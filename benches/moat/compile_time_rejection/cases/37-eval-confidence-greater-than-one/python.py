"""Python — confidence value range is convention."""
from pydantic import BaseModel


class EvalAssertion(BaseModel):
    expr_truthy: bool
    confidence: float
    runs: int


# BUG: confidence > 1.0 accepted by pydantic (no range constraint).
_assertion = EvalAssertion(expr_truthy=True, confidence=1.5, runs=5)
