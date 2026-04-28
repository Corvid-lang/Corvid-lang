"""Python equivalent — passes mypy --strict + pydantic. The bug ships."""

from typing import Annotated
from pydantic import BaseModel, Field


class Receipt(BaseModel):
    id: str


class RefundRequest(BaseModel):
    order_id: str
    amount: Annotated[float, Field(gt=0)]


# Tool annotated "dangerous" via a docstring — there is no compile-time
# representation Python+pydantic+mypy can enforce. A team-level convention
# might require a code-review check, but the type system has nothing to say.
def issue_refund(order_id: str, amount: float) -> Receipt:
    """DANGEROUS: financial impact, irreversible. Requires human approval."""
    return Receipt(id=f"r-{order_id}")


def refund_bot(req: RefundRequest) -> Receipt:
    # BUG: dangerous tool called without an approval check.
    # mypy --strict accepts this. pydantic accepts this. The bug ships.
    return issue_refund(req.order_id, req.amount)
