"""Runtime configuration — model selection and feature flags.

Precedence (highest to lowest):
  1. Per-call argument to `llm_call(..., model=...)`.
  2. `CORVID_MODEL` environment variable.
  3. `corvid.toml` file in the current working directory.

No default is hardcoded. If none of the above provides a model, the
runtime raises `NoModelConfigured`.
"""

from __future__ import annotations

import os
import tomllib
from pathlib import Path


def resolve_model(explicit: str | None) -> str | None:
    """Return the model to use, honoring the precedence rules above.

    Returns `None` if none is configured. The caller converts that into
    a `NoModelConfigured` error.
    """
    if explicit:
        return explicit
    env = os.environ.get("CORVID_MODEL")
    if env:
        return env
    return _from_toml()


def _from_toml() -> str | None:
    path = Path.cwd() / "corvid.toml"
    if not path.exists():
        return None
    try:
        data = tomllib.loads(path.read_text())
    except Exception:
        return None
    llm = data.get("llm")
    if not isinstance(llm, dict):
        return None
    model = llm.get("default_model")
    return model if isinstance(model, str) else None


def approve_all() -> bool:
    """In test/CI mode, auto-approve every approve_gate call."""
    return os.environ.get("CORVID_APPROVE_ALL") == "1"


def trace_dir() -> Path:
    """Directory where trace JSONL files land. Defaults to target/trace."""
    env = os.environ.get("CORVID_TRACE_DIR")
    if env:
        return Path(env)
    return Path.cwd() / "target" / "trace"
