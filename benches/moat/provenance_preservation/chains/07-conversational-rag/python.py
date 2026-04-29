"""LangChain ConversationalRetrievalChain. ChatMessageHistory
stores plain text messages — sources from prior turns are not
preserved. The final reply is a string with no typed sources field.
"""

from __future__ import annotations

from pydantic import BaseModel


class Doc(BaseModel):
    id: str
    text: str


def retrieve(query: str) -> Doc:
    return Doc(id=f"doc-{abs(hash(query)) % 100}", text=f"hit for {query}")


def reply(history: list[str], turn: Doc) -> str:
    """ChatMessageHistory hands you list[str]; the retrieved turn's
    sources are mentioned in the prompt but not returned typed."""
    joined = " | ".join(history)
    return f"reply given {joined} and {turn.text}"


def conversational_rag(history: list[str], query: str) -> str:
    history_text = [retrieve(h).text for h in history]
    turn_doc = retrieve(query)
    return reply(history_text, turn_doc)
