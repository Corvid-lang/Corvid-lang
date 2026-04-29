"""Python — negative confidence accepted."""
from pydantic import BaseModel


class EvalAssertion(BaseModel):
    expr_truthy: bool
    confidence: float
    runs: int


# BUG: negative confidence accepted.
_assertion = EvalAssertion(expr_truthy=True, confidence=-0.1, runs=5)
