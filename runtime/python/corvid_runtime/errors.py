"""Exception types raised by the Corvid runtime."""

from __future__ import annotations


class CorvidError(Exception):
    """Base class for all runtime errors."""


class NoModelConfigured(CorvidError):
    """`llm_call` invoked with no model set anywhere.

    Set `CORVID_MODEL`, put a `default_model` in `corvid.toml`, or pass
    `model=...` to the call.
    """


class UnknownTool(CorvidError):
    """A tool name was called that no Python implementation registered."""


class UnknownPrompt(CorvidError):
    """A prompt name was called that wasn't registered by the compiler."""


class UnknownModel(CorvidError):
    """Requested model prefix doesn't map to any adapter."""


class ApprovalDenied(CorvidError):
    """The approver rejected a dangerous action."""


class ApprovalTimeout(CorvidError):
    """No approval response arrived within the configured window."""


class DangerousCallWithoutApprove(CorvidError):
    """A dangerous tool was called at runtime without a prior approve.

    The compiler normally catches this at build time; this runtime check
    guards against bypasses (e.g. handwritten Python that imports the
    runtime directly).
    """
