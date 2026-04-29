"""Python — trust dimension lost across helper chain."""
from typing import Literal


TrustLevel = Literal["autonomous", "human_required"]


def deep_op(x: str) -> str:
    return x


def layer_one(x: str) -> str:
    return deep_op(x)


def layer_two(x: str) -> str:
    return layer_one(x)


def layer_three(x: str, declared_trust: TrustLevel = "autonomous") -> str:
    # BUG: human_required surfaces 3 layers down; outer claims autonomous.
    return layer_two(x)
