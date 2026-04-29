"""Python — approval label match is a runtime string compare, not typed."""
from pydantic import BaseModel


class Approval(BaseModel):
    label: str


def issue_refund(id: str, amount: float, approval: Approval | None = None) -> str:
    if approval is None or approval.label != "IssueRefund":
        raise PermissionError(f"approval label mismatch")
    return f"r-{id}"


def bot(id: str, amount: float) -> str:
    # BUG: label mismatch shipped because string compare is runtime-only.
    return issue_refund(id, amount, approval=Approval(label="RefundIssue"))
