"""Python — three convention checks all elsewhere."""
from pydantic import BaseModel


class Approval(BaseModel):
    label: str


class Grounded(BaseModel):
    value: str
    sources: list[str] = []


def issue_refund(id: str, approval: Approval | None = None) -> str:
    if approval is None:
        raise PermissionError("issue_refund requires approval")
    return f"r-{id}"


def triage(id: str, budget_usd: float = 0.10) -> Grounded:
    # BUG: 3 contract violations; mypy passes.
    return Grounded(value=issue_refund(id, approval=Approval(label="x")))
