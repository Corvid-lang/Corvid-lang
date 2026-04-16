"""Lightweight trace writer.

Every tool call, prompt call, approval, and agent-run boundary is appended
as a JSON line to `target/trace/<run_id>.jsonl`. Later work will build a
richer viewer; for v0.1 the file itself is the product.
"""

from __future__ import annotations

import contextvars
import json
import time
import uuid
from pathlib import Path
from typing import Any

from . import config

_current_run: contextvars.ContextVar[str | None] = contextvars.ContextVar(
    "_current_run", default=None
)


def start_run(name: str) -> str:
    """Begin a new run; subsequent events land in this run's trace file."""
    run_id = f"run_{uuid.uuid4().hex[:12]}"
    _current_run.set(run_id)
    _append({"kind": "run.start", "run_id": run_id, "agent": name})
    return run_id


def end_run(run_id: str, result: Any = None, error: str | None = None) -> None:
    event: dict[str, Any] = {"kind": "run.end", "run_id": run_id}
    if error is not None:
        event["error"] = error
    else:
        event["result"] = _to_jsonable(result)
    _append(event)
    _current_run.set(None)


def record(kind: str, **fields: Any) -> None:
    event = {"kind": kind, "ts": _now_iso(), **{k: _to_jsonable(v) for k, v in fields.items()}}
    run_id = _current_run.get()
    if run_id is not None:
        event["run_id"] = run_id
    _append(event)


def _append(event: dict[str, Any]) -> None:
    event.setdefault("ts", _now_iso())
    path = _trace_path()
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("a") as f:
            f.write(json.dumps(event) + "\n")
    except OSError:
        # Tracing must not crash user code. Silently swallow IO errors.
        pass


def _trace_path() -> Path:
    run_id = _current_run.get() or "default"
    return config.trace_dir() / f"{run_id}.jsonl"


def _now_iso() -> str:
    t = time.time()
    # ISO-8601 with millisecond precision.
    return time.strftime("%Y-%m-%dT%H:%M:%S", time.gmtime(t)) + f".{int((t % 1) * 1000):03d}Z"


def _to_jsonable(value: Any) -> Any:
    """Best-effort conversion to a JSON-serializable value."""
    try:
        json.dumps(value)
        return value
    except TypeError:
        return repr(value)
