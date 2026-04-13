"""Tool implementations + mock LLM for the refund_bot demo.

This file is imported by the generated Python. It registers mocked
implementations so the demo runs without a real database or LLM API.
"""

from __future__ import annotations

import asyncio
import os
from dataclasses import dataclass

# Runtime imports.
from corvid_runtime import llm, register_adapter, register_prompts, run_sync, tool
from corvid_runtime import testing as _testing


# ---------- mocked data ----------

_ORDERS = {
    "ord_42": {"id": "ord_42", "amount": 129.99, "user_id": "user_1"},
    "ord_43": {"id": "ord_43", "amount": 5.00, "user_id": "user_2"},
}


@dataclass
class _Order:
    id: str
    amount: float
    user_id: str


@dataclass
class _Receipt:
    refund_id: str
    amount: float


@dataclass
class _Decision:
    should_refund: bool
    reason: str


# ---------- tool implementations ----------


@tool("get_order")
async def get_order(order_id: str) -> _Order:
    raw = _ORDERS.get(order_id)
    if raw is None:
        raise KeyError(f"no order `{order_id}`")
    return _Order(**raw)


@tool("issue_refund")
async def issue_refund(order_id: str, amount: float) -> _Receipt:
    # Pretend we call Stripe.
    return _Receipt(refund_id=f"rf_{order_id}", amount=amount)


# ---------- mock LLM adapter ----------


async def _fake_llm(model: str, prompt_text: str):
    # Always refund in this demo. Real code would call the model.
    return _Decision(should_refund=True, reason="user reported legitimate complaint")


# Register under empty prefix so it wins for any model name.
register_adapter("", _fake_llm)


# ---------- approve any action (for the demo) ----------

os.environ.setdefault("CORVID_APPROVE_ALL", "1")

# Set a placeholder model so the no-default-model check passes. The mock
# adapter above is registered under the empty prefix so it serves any model.
os.environ.setdefault("CORVID_MODEL", "mock-model-for-demo")


# ---------- main ----------


def _main():
    # The generated module is target/py/refund_bot.py. It imports
    # register_prompts/register_tools from corvid_runtime and registers
    # the metadata when imported. We import it here, then run the agent.
    from pathlib import Path
    import sys

    here = Path(__file__).parent.resolve()
    sys.path.insert(0, str(here / "target" / "py"))

    from refund_bot import refund_bot  # type: ignore

    ticket = type("Ticket", (), {
        "order_id": "ord_42",
        "user_id": "user_1",
        "message": "Package never arrived",
    })()

    decision = run_sync(refund_bot, ticket)
    print(f"refund_bot decided: should_refund={decision.should_refund} "
          f"reason={decision.reason!r}")


if __name__ == "__main__":
    _main()
