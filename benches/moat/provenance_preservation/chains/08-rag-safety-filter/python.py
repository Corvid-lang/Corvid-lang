"""LangChain ConstitutionalChain + RetrievalQA. The constitutional
filter runs on plain text and returns plain text — the doc.id
that the filtered content came from is not threaded through.
"""

from __future__ import annotations

from pydantic import BaseModel


class Doc(BaseModel):
    id: str
    text: str


def retrieve(query: str) -> Doc:
    return Doc(id="doc-9", text=f"raw text for {query}")


def safety_filter(text: str) -> str:
    """Filtered string. The doc-id origin is not preserved."""
    return text.replace("RAW", "[redacted]")


def summarise(text: str) -> str:
    return f"summary of {text}"


def rag_safety_filter(query: str) -> str:
    raw = retrieve(query)
    filtered = safety_filter(raw.text)
    return summarise(filtered)
