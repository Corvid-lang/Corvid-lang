"""LangChain extraction + translation. The extracted entities lose
their source binding — they are plain strings by the time the
translator receives them.
"""

from __future__ import annotations

from pydantic import BaseModel


class Doc(BaseModel):
    id: str
    text: str


def retrieve(query: str) -> Doc:
    return Doc(id="doc-0", text=f"news article about {query}")


def extract_entities(doc: Doc) -> list[str]:
    """LangChain ExtractionChain returns strings; doc.id is dropped."""
    return [f"entity-{i}-from-{doc.text}" for i in range(3)]


def translate(entity: str) -> str:
    """Translation returns plain strings."""
    return f"es:{entity}"


# Final return type is list[str]. No typed sources field; a consumer
# cannot trace each translated entity back to a source doc.
def extract_then_translate(query: str) -> list[str]:
    doc = retrieve(query)
    entities = extract_entities(doc)
    return [translate(e) for e in entities]
