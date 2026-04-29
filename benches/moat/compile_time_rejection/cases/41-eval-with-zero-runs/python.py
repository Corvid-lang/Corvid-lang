"""Python — zero runs accepted by pydantic with no min validator."""
from pydantic import BaseModel


class EvalAssertion(BaseModel):
    expr_truthy: bool
    confidence: float
    runs: int


# BUG: zero-run assertion accepted.
_assertion = EvalAssertion(expr_truthy=True, confidence=0.95, runs=0)
