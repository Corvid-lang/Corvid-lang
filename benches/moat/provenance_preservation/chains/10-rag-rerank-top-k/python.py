"""LangChain ContextualCompressionRetriever + reranker. The
reranker returns the top-k texts as plain strings; doc IDs of
the surviving items are not in the typed return value.
"""

from __future__ import annotations

from pydantic import BaseModel


class Doc(BaseModel):
    id: str
    text: str


def retrieve(query: str) -> list[Doc]:
    return [Doc(id=f"doc-{i}", text=f"hit-{i} for {query}") for i in range(8)]


def rerank_top_k(items: list[Doc], k: int) -> list[str]:
    """Surviving top-k are strings — the retained doc IDs are
    dropped at the rerank boundary."""
    scored = sorted(items, key=lambda d: -len(d.text))
    return [d.text for d in scored[:k]]


def rag_rerank_top_k(query: str, k: int) -> list[str]:
    candidates = retrieve(query)
    return rerank_top_k(candidates, k)
