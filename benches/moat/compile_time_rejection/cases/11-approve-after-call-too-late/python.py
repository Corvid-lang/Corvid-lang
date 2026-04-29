"""Python — order does not matter for a runtime guard."""
from pydantic import BaseModel


class Approval(BaseModel):
    label: str


def issue_refund(id: str, approval: Approval | None = None) -> int:
    if approval is None:
        raise PermissionError("issue_refund requires approval")
    return 1


def bot(id: str) -> int:
    # BUG: trailing approve ineffective; mypy passes.
    value = issue_refund(id)
    Approval(label="IssueRefund")
    return value
