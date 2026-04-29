"""LangChain create_structured_output_chain + pydantic Product
schema. The Product model has name/price/description but no
sources field — the typed return value is provenance-free.
"""

from __future__ import annotations

from pydantic import BaseModel


class Doc(BaseModel):
    id: str
    text: str


class Product(BaseModel):
    name: str
    price: float
    description: str


def retrieve(query: str) -> Doc:
    return Doc(id="doc-3", text=f"product blurb for {query}")


def extract_product(doc: Doc) -> Product:
    return Product(name="widget", price=9.99, description=doc.text)


def rag_to_json_record(query: str) -> Product:
    doc = retrieve(query)
    return extract_product(doc)
