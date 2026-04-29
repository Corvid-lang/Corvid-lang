"""Python — helper-side approval is not visible to the type system."""
from pydantic import BaseModel


class Approval(BaseModel):
    label: str


def issue_refund(id: str, approval: Approval | None = None) -> int:
    if approval is None:
        raise PermissionError("issue_refund requires approval")
    return 1


def helper(id: str) -> int:
    return issue_refund(id, approval=Approval(label="IssueRefund"))


def outer(id: str) -> int:
    a = helper(id)
    # BUG: missing runtime guard at the outer site.
    b = issue_refund(id)
    return a + b
