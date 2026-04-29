"""RAG-QA-bot reference implementation — Python.

A bot that answers user questions over an internal corpus and can
share a source document directly with the user (the dangerous
action). Every governance check is application code: an `if` to
gate a budget, a manual citation list, an audit-log write, an
approval token whose trust level is enforced by hand.

Lines tagged `# governance` in trailing comments are what the
counter classifies. Removing any of them ships a real bug class.
"""

from __future__ import annotations

import functools
import logging
import time
from dataclasses import field
from enum import Enum
from typing import Annotated, Callable, TypeVar

from pydantic import BaseModel, Field, ValidationError  # governance


_AUDIT_LOG: list[dict[str, object]] = []  # governance


class TrustLevel(str, Enum):  # governance
    AUTONOMOUS = "autonomous"
    SUPERVISOR = "supervisor"
    HUMAN_REQUIRED = "human_required"


class Question(BaseModel):
    user_id: str
    text: str


class Answer(BaseModel):
    text: str
    sources: list[str] = field(default_factory=list)  # governance (provenance)


F = TypeVar("F", bound=Callable[..., object])


def dangerous(trust: TrustLevel) -> Callable[[F], F]:  # governance
    """Convention-only `dangerous` decorator. Removing the approval
    check or the trust check ships a real bug class."""

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
def share_source(doc_id: str, user_id: str) -> str:
    return f"shared:{doc_id}:to:{user_id}"


def retrieve_docs(query: str) -> list[tuple[str, str]]:  # governance (return shape)
    # governance: must return (text, doc_id) pairs so callers can
    # preserve citations. Forgetting the doc_id is a real bug class.
    return [
        (f"doc-1 hit for {query}", "kb://policies/1"),  # governance
        (f"doc-2 hit for {query}", "kb://policies/2"),  # governance
    ]


def synthesize(question: str, docs: list[tuple[str, str]]) -> tuple[str, list[str]]:
    # governance: caller must thread doc_ids through to the answer
    text = f"answer to '{question}' using {len(docs)} sources"
    sources = [doc_id for _, doc_id in docs]  # governance
    return text, sources


_BUDGET_USD: dict[str, float] = {}  # governance (per-user budget cap)


def answer_question(q: Question) -> Answer:
    spent = _BUDGET_USD.get(q.user_id, 0.0)  # governance
    if spent + 0.10 > 10.0:  # governance (budget cap)
        raise PermissionError(f"budget exceeded for {q.user_id}")  # governance
    _BUDGET_USD[q.user_id] = spent + 0.10  # governance
    docs = retrieve_docs(q.text)
    text, sources = synthesize(q.text, docs)
    if not sources:  # governance (catches dropped citation)
        raise ValueError("ungrounded answer rejected")  # governance
    return Answer(text=text, sources=sources)


def share_source_doc(doc_id: str, user_id: str) -> str:
    approval = {"trust": TrustLevel.HUMAN_REQUIRED.value, "actor": "human"}  # governance
    return str(share_source(doc_id, user_id, approval=approval))  # type: ignore[call-arg]
