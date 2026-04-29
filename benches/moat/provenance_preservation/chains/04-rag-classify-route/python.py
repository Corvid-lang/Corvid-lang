"""LangChain-style RouterChain. Classifier returns a plain string
label; the route function returns a plain str answer. The doc.id
metadata never reaches the final return type.
"""

from __future__ import annotations

from pydantic import BaseModel


class Doc(BaseModel):
    id: str
    text: str


def retrieve(query: str) -> Doc:
    return Doc(id="doc-7", text=f"hit for {query}")


def classify(doc: Doc) -> str:
    return "billing" if "invoice" in doc.text else "support"


def billing_specialist(doc: Doc) -> str:
    return f"billing reply for {doc.text}"


def support_specialist(doc: Doc) -> str:
    return f"support reply for {doc.text}"


def rag_classify_route(query: str) -> str:
    doc = retrieve(query)
    topic = classify(doc)
    if topic == "billing":
        return billing_specialist(doc)
    return support_specialist(doc)
