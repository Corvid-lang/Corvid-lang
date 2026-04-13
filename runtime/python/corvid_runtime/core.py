"""The three functions generated Python calls into.

* `tool_call(name, args)` — dispatch to a registered Python implementation.
* `approve_gate(label, args)` — re-exported from `approvals` for a clean API.
* `llm_call(name, args, model=None)` — dispatch a prompt through an adapter.

Also provides `run(agent_fn, *args)` — a convenience wrapper that starts a
trace, invokes the agent, and closes the trace.
"""

from __future__ import annotations

import asyncio
from typing import Any, Awaitable, Callable

from . import tracing
from .approvals import approve_gate as approve_gate  # re-export
from .errors import DangerousCallWithoutApprove
from .llm import llm_call as llm_call  # re-export
from .registry import get_tool_impl, get_tool_meta


async def tool_call(name: str, args: list[Any]) -> Any:
    """Dispatch a call to the Python implementation registered for `name`.

    Records the call in the trace with its effect classification. Dangerous
    calls are a compile-time error for Corvid programs; this runtime check
    catches them only if something has bypassed the compiler.
    """
    meta = get_tool_meta(name)
    effect = (meta or {}).get("effect", "safe")

    tracing.record("tool.call", name=name, effect=effect, args=args)
    impl = get_tool_impl(name)
    try:
        result = await impl(*args)
    except Exception as exc:
        tracing.record("tool.error", name=name, message=repr(exc))
        raise
    tracing.record("tool.result", name=name)
    return result


async def run(agent_fn: Callable[..., Awaitable[Any]], *args: Any) -> Any:
    """Run an agent under a fresh trace.

    Typical usage from a CLI or user script:

        from my_project.target.py.refund_bot import refund_bot
        from corvid_runtime import run
        result = asyncio.run(run(refund_bot, ticket))
    """
    run_id = tracing.start_run(agent_fn.__name__)
    try:
        result = await agent_fn(*args)
    except Exception as e:
        tracing.end_run(run_id, error=repr(e))
        raise
    tracing.end_run(run_id, result=result)
    return result


def run_sync(agent_fn: Callable[..., Awaitable[Any]], *args: Any) -> Any:
    """Synchronous wrapper for `run()` — convenient for scripts and tests."""
    return asyncio.run(run(agent_fn, *args))


# Re-export helpers so `from corvid_runtime import ...` is frictionless.
__all__ = [
    "tool_call",
    "approve_gate",
    "llm_call",
    "run",
    "run_sync",
    "DangerousCallWithoutApprove",
]
