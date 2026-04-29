"""Python — runtime flag visible only at runtime."""
from pydantic import BaseModel


class Approval(BaseModel):
    label: str


def issue_refund(id: str, approval: Approval | None = None) -> int:
    if approval is None:
        raise PermissionError("issue_refund requires approval")
    return 1


def bot(id: str, debug: bool) -> int:
    approval: Approval | None = None
    if debug:
        approval = Approval(label="IssueRefund")
    # BUG: when debug is False, approval is None — runtime crash, not compile error.
    return issue_refund(id, approval=approval)
