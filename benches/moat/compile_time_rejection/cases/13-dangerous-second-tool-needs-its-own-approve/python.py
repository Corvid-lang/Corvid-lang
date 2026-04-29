"""Python — approval scoping is convention-only."""
from pydantic import BaseModel


class Approval(BaseModel):
    label: str


def issue_refund(id: str, approval: Approval | None = None) -> int:
    if approval is None:
        raise PermissionError("issue_refund requires approval")
    return 1


def wire_transfer(id: str, amount: float, approval: Approval | None = None) -> int:
    if approval is None or approval.label != "WireTransfer":
        raise PermissionError("wire_transfer requires its own approval")
    return 1


def bot(id: str, amount: float) -> int:
    approval = Approval(label="IssueRefund")
    # BUG: same approval reused for a different dangerous tool.
    return wire_transfer(id, amount, approval=approval)
