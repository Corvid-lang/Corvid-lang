"""LangChain Tool-augmented RAG. The tool call drops the source
binding when it returns a string; the answer chain has no way to
trace back to the original retrieval.
"""

from __future__ import annotations

from pydantic import BaseModel


class Doc(BaseModel):
    id: str
    text: str


def retrieve(query: str) -> Doc:
    return Doc(id="doc-0", text=f"primary doc for {query}")


def enrich_from_db(doc: Doc) -> str:
    """LangChain Tool returns a string; doc.id is dropped."""
    return f"related-record-for-{doc.text}"


def answer(doc: Doc, related: str) -> str:
    """LLMChain returns a string; sources gone."""
    return f"answer using {doc.text} and related {related}"


# Final return type is `str`. No typed sources field; a consumer
# cannot trace the answer back to either the original doc OR the
# enrichment record without manual threading.
def retrieve_enrich_answer(query: str) -> str:
    doc = retrieve(query)
    related = enrich_from_db(doc)
    return answer(doc, related)
