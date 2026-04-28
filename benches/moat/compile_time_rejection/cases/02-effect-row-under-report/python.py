"""Python equivalent — passes mypy --strict. Effect row drift ships."""

from typing import Annotated, Literal
from pydantic import BaseModel, Field


class Receipt(BaseModel):
    id: str


# Effects are convention-only in Python. A team might use `enum`s + a
# decorator, but no static checker enforces that a caller's declared
# effect set covers its callees'. mypy has nothing to say.
TrustLevel = Literal["autonomous", "supervisor_required", "human_required"]


def issue_refund(
    order_id: str,
    amount: Annotated[float, Field(gt=0)],
    trust: TrustLevel = "human_required",
) -> Receipt:
    return Receipt(id=f"r-{order_id}")


def helper(
    order_id: str,
    amount: float,
    trust: TrustLevel = "human_required",
) -> Receipt:
    # Helper inherits the human_required trust level from the tool.
    return issue_refund(order_id, amount, trust)


# BUG: outer's declared trust ("autonomous") is narrower than helper's
# actual produced trust ("human_required"). mypy passes. The drift
# ships into prod.
def outer(
    order_id: str,
    amount: float,
    trust: TrustLevel = "autonomous",
) -> Receipt:
    return helper(order_id, amount)
