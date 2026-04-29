"""LangChain LLMChain + statistics. Per-doc extraction returns
plain floats; the sum is a plain float with no source binding.
"""

from __future__ import annotations

from pydantic import BaseModel


class Doc(BaseModel):
    id: str
    text: str


def retrieve(query: str) -> list[Doc]:
    return [Doc(id=f"doc-{i}", text=f"hit-{i} for {query} costs ${10 * i + 5}") for i in range(3)]


def extract_amount(doc: Doc) -> float:
    """LLMChain returns a number; doc.id origin is dropped."""
    return float(len(doc.text)) % 100


def sum_amounts(values: list[float]) -> float:
    return sum(values)


def rag_numeric_aggregation(query: str) -> float:
    docs = retrieve(query)
    amounts = [extract_amount(d) for d in docs]
    return sum_amounts(amounts)
