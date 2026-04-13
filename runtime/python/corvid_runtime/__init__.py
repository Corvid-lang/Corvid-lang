"""Corvid runtime — the Python library that generated .cor programs call into.

Public surface:
    tool_call, approve_gate, llm_call           # called by generated code
    tool                                         # @tool decorator for users
    register_tools, register_prompts             # called by generated modules
    run, run_sync                                # run an agent under a trace
    CorvidError and subclasses                   # runtime errors
    testing                                      # test helpers
"""

from __future__ import annotations

from .approvals import approve_gate, set_approver
from .core import run, run_sync, tool_call
from .errors import (
    ApprovalDenied,
    ApprovalTimeout,
    CorvidError,
    DangerousCallWithoutApprove,
    NoModelConfigured,
    UnknownModel,
    UnknownPrompt,
    UnknownTool,
)
from .llm import llm_call, register_adapter
from .registry import register_prompts, register_tools, tool

__all__ = [
    "approve_gate",
    "llm_call",
    "register_adapter",
    "register_prompts",
    "register_tools",
    "run",
    "run_sync",
    "set_approver",
    "tool",
    "tool_call",
    # errors
    "ApprovalDenied",
    "ApprovalTimeout",
    "CorvidError",
    "DangerousCallWithoutApprove",
    "NoModelConfigured",
    "UnknownModel",
    "UnknownPrompt",
    "UnknownTool",
]
