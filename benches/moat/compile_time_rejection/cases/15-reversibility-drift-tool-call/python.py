"""Python — reversibility is convention-only."""
from pydantic import BaseModel


class Approval(BaseModel):
    label: str


def issue_refund(id: str, approval: Approval, reversible: bool = False) -> int:
    return 1


def bot(id: str, declared_reversible: bool = True) -> int:
    # BUG: declared reversible=True but tool returns irreversible op.
    return issue_refund(id, Approval(label="IssueRefund"))
