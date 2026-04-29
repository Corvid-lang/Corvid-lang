"""Python — manual retry loop; runtime guard, no compile-time enforcement."""
from pydantic import BaseModel


class Approval(BaseModel):
    label: str


def issue_refund(id: str, approval: Approval | None = None) -> int:
    if approval is None:
        raise PermissionError("issue_refund requires approval")
    return 1


def bot(id: str) -> int:
    last_err: Exception | None = None
    for _ in range(3):
        try:
            # BUG: retry body has no runtime guard.
            return issue_refund(id)
        except Exception as err:
            last_err = err
    raise last_err if last_err else RuntimeError("retry exhausted")
