"""Python — trust dimension is convention-only."""
from typing import Literal
from pydantic import BaseModel


TrustLevel = Literal["autonomous", "supervisor", "human_required"]


class Approval(BaseModel):
    label: str


def issue_refund(
    id: str, approval: Approval, trust: TrustLevel = "human_required"
) -> int:
    return 1


def bot(id: str, declared_trust: TrustLevel = "autonomous") -> int:
    # BUG: tool's required trust ("human_required") narrower than declared.
    return issue_refund(id, Approval(label="IssueRefund"))
