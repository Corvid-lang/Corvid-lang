"""Python — approval is convention-only, arity is not enforced."""
from pydantic import BaseModel


class Approval(BaseModel):
    label: str
    args: list[str]


def send_email(to: str, body: str, approval: Approval | None = None) -> None:
    if approval is None or len(approval.args) != 2:
        raise PermissionError("approval arity mismatch")


def notify(to: str) -> None:
    # BUG: arity 1 but tool takes 2; mypy passes.
    send_email(to, to, approval=Approval(label="SendEmail", args=[to]))
