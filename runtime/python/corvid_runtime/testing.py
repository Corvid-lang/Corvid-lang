"""Helpers for testing Corvid-generated code without hitting the network."""

from __future__ import annotations

from typing import Any, Awaitable, Callable

from . import approvals, llm, registry


def mock_llm(responses: dict[str, Any]) -> None:
    """Install a fake adapter that returns canned results by prompt name.

    The prompt's `template` is checked for each key; the first substring
    match wins. This is intentionally simple — for anything more elaborate,
    call `llm.register_adapter(...)` directly.
    """

    async def fake(_model: str, prompt_text: str) -> Any:
        for key, value in responses.items():
            if key in prompt_text:
                return value
        return None

    # Register under a broad prefix so it wins for any model.
    llm.register_adapter("", fake)


def mock_approve_all(answer: bool = True) -> None:
    """Auto-decide every approval with the given outcome."""

    async def approver(_label: str, _args: list[Any]) -> bool:
        return answer

    approvals.set_approver(approver)


def reset() -> None:
    """Clear all registered state (useful between tests)."""
    registry.reset_for_testing()
    approvals.set_approver(None)  # type: ignore[arg-type]
    # LLM adapters deliberately not cleared — Anthropic's registration is
    # done at import time.
