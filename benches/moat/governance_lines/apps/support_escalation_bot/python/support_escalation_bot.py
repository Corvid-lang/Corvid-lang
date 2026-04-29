"""Support-escalation-bot reference implementation — Python.

Same product as the Corvid version: triage incoming tickets, page
on-call humans for severe ones. Paging is the dangerous action;
the triage rationale must carry provenance back to the ticket
history. Every governance check is application code with a
trailing `# governance` marker.
"""

from __future__ import annotations

import functools
import logging
import time
from collections import defaultdict
from dataclasses import field
from enum import Enum
from typing import Annotated, Callable, TypeVar

from pydantic import BaseModel, Field, ValidationError  # governance


_AUDIT_LOG: list[dict[str, object]] = []  # governance


class TrustLevel(str, Enum):  # governance
    AUTONOMOUS = "autonomous"
    SUPERVISOR = "supervisor"
    HUMAN_REQUIRED = "human_required"


class Ticket(BaseModel):
    id: str
    customer_id: str
    body: str
    severity: str


class Triage(BaseModel):
    decision: str
    rationale: str
    sources: list[str] = field(default_factory=list)  # governance (provenance)


F = TypeVar("F", bound=Callable[..., object])


def dangerous(trust: TrustLevel) -> Callable[[F], F]:  # governance
    """Convention-only `dangerous` decorator."""

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
                    f"trust level mismatch for {fn.__name__}"  # governance
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
def escalate_to_oncall(ticket_id: str, severity: str) -> str:
    return f"paged:{ticket_id}:sev={severity}"


def fetch_history(customer_id: str) -> list[tuple[str, str]]:  # governance (return shape)
    # governance: must return (text, source_id) so callers can preserve citations.
    return [
        (f"prior ticket A for {customer_id}", f"db://tickets/{customer_id}/a"),  # governance
        (f"prior ticket B for {customer_id}", f"db://tickets/{customer_id}/b"),  # governance
    ]


def classify_severity(
    body: str, history: list[tuple[str, str]]
) -> tuple[str, list[str]]:
    decision = "high" if "outage" in body.lower() else "normal"
    sources = [src for _, src in history]  # governance (sources thread)
    return decision, sources


_BUDGET_USD: dict[str, float] = defaultdict(float)  # governance (per-customer budget cap)
_ESCALATIONS_PER_HOUR: dict[str, int] = defaultdict(int)  # governance (rate limit)


def triage_ticket(t: Ticket) -> Triage:
    spent = _BUDGET_USD[t.customer_id]  # governance
    if spent + 0.10 > 5.0:  # governance (budget cap)
        raise PermissionError(f"budget exceeded for {t.customer_id}")  # governance
    _BUDGET_USD[t.customer_id] = spent + 0.10  # governance
    history = fetch_history(t.customer_id)
    decision, sources = classify_severity(t.body, history)
    if not sources:  # governance (catches dropped citation)
        raise ValueError("ungrounded triage rejected")  # governance
    return Triage(decision=decision, rationale=decision, sources=sources)


def escalate(t: Ticket, severity: str) -> str:
    if _ESCALATIONS_PER_HOUR[t.customer_id] >= 5:  # governance (rate limit)
        raise PermissionError("escalation rate exceeded")  # governance
    _ESCALATIONS_PER_HOUR[t.customer_id] += 1  # governance
    approval = {"trust": TrustLevel.HUMAN_REQUIRED.value, "actor": "human"}  # governance
    return str(escalate_to_oncall(t.id, severity, approval=approval))  # type: ignore[call-arg]
