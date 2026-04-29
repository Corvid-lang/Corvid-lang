"""Python — pytest-style mock; runtime guard removed by the mock,
no language-level approval invariant."""
from pydantic import BaseModel


class Approval(BaseModel):
    label: str


def issue_refund(id: str, approval: Approval | None = None) -> int:
    if approval is None:
        raise PermissionError("issue_refund requires approval")
    return 42


# The mocked replacement skips the runtime guard; tests use it freely.
def fake_issue_refund(id: str, approval: Approval | None = None) -> int:
    return 42


def test_unsafe_call() -> None:
    # BUG: the mock erases the dangerous marker at runtime; mypy passes.
    value = fake_issue_refund("o-1")
    assert value == 42
