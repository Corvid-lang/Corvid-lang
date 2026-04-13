"""Approval gate for dangerous tool calls.

Default mode is interactive: print the action, wait for a y/N response
on stdin. Set `CORVID_APPROVE_ALL=1` for CI / tests to auto-approve.

A programmatic handler can also be installed via `set_approver(fn)`.
"""

from __future__ import annotations

import sys
from typing import Any, Awaitable, Callable

from . import config, tracing
from .errors import ApprovalDenied


# Signature: (label: str, args: list) -> bool (True = approve)
Approver = Callable[[str, list[Any]], Awaitable[bool]]

_approver: Approver | None = None


def set_approver(fn: Approver) -> None:
    """Replace the default interactive approver (for tests, web UIs, etc.)."""
    global _approver
    _approver = fn


async def approve_gate(label: str, args: list[Any]) -> None:
    """Called by generated code at every `approve Label(args)` statement.

    Blocks until approval is granted. Raises `ApprovalDenied` on rejection.
    Emits a trace event for both outcomes.
    """
    tracing.record("approve.request", label=label, args=args)

    if _approver is not None:
        ok = await _approver(label, args)
    elif config.approve_all():
        ok = True
    else:
        ok = _default_interactive(label, args)

    if ok:
        tracing.record("approve.resolved", label=label, decision="approve")
    else:
        tracing.record("approve.resolved", label=label, decision="deny")
        raise ApprovalDenied(f"action `{label}` was rejected")


def _default_interactive(label: str, args: list[Any]) -> bool:
    """Print the action and block on stdin. Good enough for local dev."""
    args_str = ", ".join(repr(a) for a in args)
    print(f"\n[corvid] approval required: {label}({args_str})", file=sys.stderr)
    print("  approve? [y/N] ", end="", file=sys.stderr, flush=True)
    answer = sys.stdin.readline().strip().lower()
    return answer == "y" or answer == "yes"
