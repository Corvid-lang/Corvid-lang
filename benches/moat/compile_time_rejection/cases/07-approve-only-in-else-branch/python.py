"""Python — uniform runtime guard, mypy passes."""
from pydantic import BaseModel


class Approval(BaseModel):
    label: str


def send_email(to: str, body: str, approval: Approval | None = None) -> None:
    if approval is None:
        raise PermissionError("send_email requires approval")


def notify(flag: bool, to: str) -> None:
    if flag:
        # BUG: then-branch path has no runtime guard.
        send_email(to, to)
        return
    send_email(to, to, approval=Approval(label="SendEmail"))
