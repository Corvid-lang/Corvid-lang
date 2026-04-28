"""Refund-bot reference implementation — Python.

Every governance check in this file is *application code*: an `if`
statement, an enum lookup, a custom decorator, a runtime audit-log
write, a manual citation list. The type system has nothing to enforce;
removing any of these lines lets a real bug ship.

Lines tagged `# governance` in trailing comments are the ones the
counter classifies. The pattern is intentional — a Python developer
who removes a `# governance` line ships a real bug class.
"""

from __future__ import annotations

import functools
import logging
import time
from dataclasses import dataclass, field
from enum import Enum
from typing import Annotated, Callable, TypeVar

from pydantic import BaseModel, Field, ValidationError  # governance

# governance: trace event sink. Convention-only.
_AUDIT_LOG: list[dict[str, object]] = []  # governance


class TrustLevel(str, Enum):  # governance
    AUTONOMOUS = "autonomous"
    SUPERVISOR = "supervisor"
    HUMAN_REQUIRED = "human_required"


class RefundRequest(BaseModel):
    order_id: str
    amount: Annotated[float, Field(gt=0, le=500.0)]  # governance (budget cap)
    reason: str


class RefundResponse(BaseModel):
    receipt_id: str
    status: str


class RefundExplanation(BaseModel):
    reason: str
    sources: list[str] = field(default_factory=list)  # governance (provenance)


F = TypeVar("F", bound=Callable[..., object])


def dangerous(trust: TrustLevel) -> Callable[[F], F]:  # governance
    """Decorator marking a tool dangerous. Convention-only enforcement."""

    def decorator(fn: F) -> F:
        @functools.wraps(fn)
        def wrapper(*args: object, **kwargs: object) -> object:
            approval = kwargs.pop("approval", None)  # governance
            if approval is None:  # governance
                raise PermissionError(  # governance
                    f"{fn.__name__} requires an approval token"  # governance
                )  # governance
            if approval.get("trust") != trust.value:  # governance
                raise PermissionError(  # governance
                    f"approval trust level mismatch for {fn.__name__}"  # governance
                )  # governance
            _AUDIT_LOG.append(  # governance
                {  # governance
                    "tool": fn.__name__,  # governance
                    "approval": approval,  # governance
                    "ts": time.time(),  # governance
                }  # governance
            )  # governance
            return fn(*args, **kwargs)

        return wrapper  # type: ignore[return-value]

    return decorator


@dangerous(TrustLevel.HUMAN_REQUIRED)  # governance
def issue_refund(req: RefundRequest) -> str:
    return f"r-{req.order_id}"


def fetch_order(order_id: str) -> tuple[str, list[str]]:  # governance (return shape)
    # governance: must return a (text, sources) pair so callers can preserve
    # citations. Forgetting the second element is a real bug class.
    text = f"order {order_id} placed 2026-04-21"
    sources = [f"db://orders/{order_id}"]  # governance
    return text, sources  # governance


def approve_refund(req: RefundRequest) -> RefundResponse:
    # governance: caller must construct the approval token correctly.
    approval = {"trust": TrustLevel.HUMAN_REQUIRED.value, "actor": "human"}  # governance
    receipt_id = issue_refund(req, approval=approval)  # type: ignore[call-arg]
    return RefundResponse(receipt_id=str(receipt_id), status="approved")


def explain_refund(order_id: str) -> RefundExplanation:
    text, sources = fetch_order(order_id)
    if not sources:  # governance (catches dropped citation)
        raise ValueError("ungrounded explanation rejected")  # governance
    return RefundExplanation(reason=text, sources=sources)
