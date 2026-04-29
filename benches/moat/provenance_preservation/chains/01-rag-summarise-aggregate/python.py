"""LangChain-style multi-hop RAG. After per-doc summarisation, the
citation chain dissolves: the aggregator receives plain strings.

The final return type is `str` — to recover sources, the developer
would have to manually thread document IDs through the chain. The
LangChain `RetrievalQAWithSources` chain handles step 1 → step 4,
but inserting a per-doc summarisation hop in between breaks it.
"""

from __future__ import annotations

from pydantic import BaseModel


class Doc(BaseModel):
    id: str
    text: str


def retrieve(query: str) -> list[Doc]:
    """LangChain RetrievalQA returns Documents with metadata."""
    return [Doc(id=f"doc-{i}", text=f"hit-{i} for {query}") for i in range(3)]


def summarise(doc: Doc) -> str:
    """LLMChain over a single doc — return type is plain str.
    The doc.id metadata is dropped at this hop.
    """
    return f"summary of {doc.text}"


def aggregate(parts: list[str]) -> str:
    """Aggregator receives strings; provenance is gone."""
    return "; ".join(parts)


# The final return type is `str` — there is no typed sources field.
# A consumer cannot recover the original doc IDs from this answer
# without re-running step 1 or threading metadata manually.
def multi_hop_rag(query: str) -> str:
    docs = retrieve(query)
    summaries = [summarise(d) for d in docs]
    return aggregate(summaries)
