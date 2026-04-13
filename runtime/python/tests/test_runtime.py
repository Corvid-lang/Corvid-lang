"""Unit tests for the Corvid Python runtime."""

from __future__ import annotations

import os
from pathlib import Path

import pytest

import corvid_runtime
from corvid_runtime import (
    NoModelConfigured,
    UnknownPrompt,
    UnknownTool,
    approve_gate,
    llm_call,
    register_adapter,
    register_prompts,
    register_tools,
    tool,
    tool_call,
)
from corvid_runtime import testing as _testing
from corvid_runtime import tracing


@pytest.fixture(autouse=True)
def _clean(tmp_path, monkeypatch):
    _testing.reset()
    # Route traces into a tmpdir per test so we don't pollute the repo.
    monkeypatch.setenv("CORVID_TRACE_DIR", str(tmp_path))
    yield


# ---------------- tool_call ----------------


async def test_tool_call_dispatches_to_registered_impl():
    register_tools({"echo": {"effect": "safe", "arity": 1}})

    @tool("echo")
    async def echo(x):
        return x

    result = await tool_call("echo", ["hello"])
    assert result == "hello"


async def test_tool_call_raises_when_impl_missing():
    register_tools({"nope": {"effect": "safe", "arity": 0}})
    with pytest.raises(UnknownTool):
        await tool_call("nope", [])


# ---------------- approve_gate ----------------


async def test_approve_gate_with_programmatic_approver():
    _testing.mock_approve_all(answer=True)
    await approve_gate("IssueRefund", ["ord_42", 500.0])


async def test_approve_gate_rejection_raises():
    from corvid_runtime import ApprovalDenied

    _testing.mock_approve_all(answer=False)
    with pytest.raises(ApprovalDenied):
        await approve_gate("IssueRefund", ["ord_42", 500.0])


async def test_approve_all_env_var_approves(monkeypatch):
    monkeypatch.setenv("CORVID_APPROVE_ALL", "1")
    await approve_gate("Anything", [])


# ---------------- llm_call ----------------


async def test_llm_call_fails_without_model(tmp_path, monkeypatch):
    monkeypatch.delenv("CORVID_MODEL", raising=False)
    # Make sure we're not sitting in a directory with a corvid.toml.
    monkeypatch.chdir(tmp_path)
    register_prompts(
        {"greet": {"template": "Hi {name}.", "params": ["name"]}}
    )
    with pytest.raises(NoModelConfigured):
        await llm_call("greet", ["Ada"])


async def test_llm_call_uses_mock_adapter(monkeypatch):
    monkeypatch.setenv("CORVID_MODEL", "test-mock-1")
    register_prompts(
        {"greet": {"template": "Hi {name}.", "params": ["name"]}}
    )

    async def fake(model, prompt_text):
        assert model == "test-mock-1"
        assert "Hi Ada." in prompt_text
        return "hello Ada"

    register_adapter("test-", fake)
    result = await llm_call("greet", ["Ada"])
    assert result == "hello Ada"


async def test_llm_call_raises_on_unknown_prompt(monkeypatch):
    monkeypatch.setenv("CORVID_MODEL", "test-x")
    with pytest.raises(UnknownPrompt):
        await llm_call("not_registered", [])


# ---------------- trace file is written ----------------


async def test_tool_call_writes_trace(tmp_path, monkeypatch):
    monkeypatch.setenv("CORVID_TRACE_DIR", str(tmp_path))

    register_tools({"ping": {"effect": "safe", "arity": 0}})

    @tool("ping")
    async def ping():
        return "pong"

    tracing.start_run("test_agent")
    await tool_call("ping", [])

    files = list(tmp_path.glob("*.jsonl"))
    assert files, "expected a trace file"
    content = files[0].read_text()
    assert "tool.call" in content
    assert "tool.result" in content


# ---------------- run() under a trace ----------------


async def test_run_wraps_agent_in_trace(tmp_path, monkeypatch):
    monkeypatch.setenv("CORVID_TRACE_DIR", str(tmp_path))

    async def my_agent(n):
        return n * 2

    result = await corvid_runtime.run(my_agent, 21)
    assert result == 42

    files = list(tmp_path.glob("*.jsonl"))
    assert files
    content = files[0].read_text()
    assert "run.start" in content
    assert "run.end" in content
