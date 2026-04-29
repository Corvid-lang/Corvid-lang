"""LangChain MultiRetrievalQA + custom dedupe. The dedupe step
operates on plain strings; surviving entries don't carry their
source IDs, so the final aggregate has no typed sources field.
"""

from __future__ import annotations

from pydantic import BaseModel


class Doc(BaseModel):
    id: str
    text: str


def retrieve_wiki(query: str) -> list[Doc]:
    return [Doc(id=f"wiki-{i}", text=f"wiki-hit-{i} for {query}") for i in range(2)]


def retrieve_internal(query: str) -> list[Doc]:
    return [Doc(id=f"int-{i}", text=f"int-hit-{i} for {query}") for i in range(2)]


def dedupe(items: list[Doc]) -> list[str]:
    """Surviving items are strings — source IDs are gone."""
    seen: set[str] = set()
    out: list[str] = []
    for d in items:
        if d.text not in seen:
            seen.add(d.text)
            out.append(d.text)
    return out


def aggregate(parts: list[str]) -> str:
    return "; ".join(parts)


def multi_source_dedupe(query: str) -> str:
    merged = dedupe(retrieve_wiki(query) + retrieve_internal(query))
    return aggregate(merged)
