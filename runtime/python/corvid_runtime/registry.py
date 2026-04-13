"""Tool and prompt registries.

Users register tool implementations with `@tool("name")`. The compiler
emits a `TOOLS` dict as metadata (name → {"effect", "arity"}); the
user-provided Python functions supply the actual callable.

Prompts are registered by generated code's `PROMPTS` dict: name → {"template",
"params"}. This module holds the latest `PROMPTS` registered.
"""

from __future__ import annotations

from typing import Any, Awaitable, Callable, TypedDict

from .errors import UnknownPrompt, UnknownTool


class ToolMetadata(TypedDict):
    effect: str  # "safe" or "dangerous"
    arity: int


class PromptMetadata(TypedDict):
    template: str
    params: list[str]


# Registered Python implementations: tool name → async callable.
_TOOL_IMPLS: dict[str, Callable[..., Awaitable[Any]]] = {}

# Compiler-provided metadata. Populated by generated modules on import.
_TOOL_META: dict[str, ToolMetadata] = {}
_PROMPT_META: dict[str, PromptMetadata] = {}


def tool(name: str) -> Callable[[Callable[..., Awaitable[Any]]], Callable[..., Awaitable[Any]]]:
    """Decorator: register `fn` as the implementation of tool `name`."""

    def wrap(fn: Callable[..., Awaitable[Any]]) -> Callable[..., Awaitable[Any]]:
        _TOOL_IMPLS[name] = fn
        return fn

    return wrap


def register_tools(meta: dict[str, ToolMetadata]) -> None:
    """Called by generated modules: merge tool metadata into the registry."""
    _TOOL_META.update(meta)


def register_prompts(meta: dict[str, PromptMetadata]) -> None:
    """Called by generated modules: merge prompt metadata into the registry."""
    _PROMPT_META.update(meta)


def get_tool_impl(name: str) -> Callable[..., Awaitable[Any]]:
    try:
        return _TOOL_IMPLS[name]
    except KeyError as e:
        raise UnknownTool(
            f"no Python implementation registered for tool `{name}`.\n"
            f"  help: decorate a function with `@tool(\"{name}\")` before running this agent."
        ) from e


def get_tool_meta(name: str) -> ToolMetadata | None:
    return _TOOL_META.get(name)


def get_prompt_meta(name: str) -> PromptMetadata:
    try:
        return _PROMPT_META[name]
    except KeyError as e:
        raise UnknownPrompt(
            f"no prompt registered for `{name}`. The compiler should register "
            f"it via `register_prompts({{...}})` when the generated module is imported."
        ) from e


def reset_for_testing() -> None:
    """Wipe all registries. Test-only."""
    _TOOL_IMPLS.clear()
    _TOOL_META.clear()
    _PROMPT_META.clear()
