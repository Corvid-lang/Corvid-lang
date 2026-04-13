"""LLM call dispatch + provider adapters.

v0.1 ships with an Anthropic adapter. Other providers arrive in v0.2 as
additional adapters registered in the `_ADAPTERS` table.
"""

from __future__ import annotations

from typing import Any, Awaitable, Callable

from . import config, tracing
from .errors import NoModelConfigured, UnknownModel
from .registry import get_prompt_meta


# An adapter is: (model: str, prompt_text: str) -> awaitable[Any].
Adapter = Callable[[str, str], Awaitable[Any]]

_ADAPTERS: dict[str, Adapter] = {}


def register_adapter(prefix: str, fn: Adapter) -> None:
    """Register `fn` to handle any model name starting with `prefix`."""
    _ADAPTERS[prefix] = fn


def _pick_adapter(model: str) -> Adapter:
    for prefix, fn in _ADAPTERS.items():
        if model.startswith(prefix):
            return fn
    raise UnknownModel(
        f"no adapter registered for model `{model}`.\n"
        f"  help: install `corvid-runtime[anthropic]` for Claude support, "
        "or call `register_adapter(prefix, fn)` to plug in a custom provider."
    )


async def llm_call(prompt_name: str, args: list[Any], model: str | None = None) -> Any:
    """Dispatch a prompt call through the configured model adapter.

    `args` is the positional list of argument values corresponding to the
    prompt's declared parameters. The prompt template is rendered with
    `{name}` placeholders substituted from those args.
    """
    meta = get_prompt_meta(prompt_name)
    chosen = config.resolve_model(model)
    if chosen is None:
        raise NoModelConfigured(
            "no model configured.\n"
            "  help: set `CORVID_MODEL=...` in the environment, add "
            "`default_model = \"...\"` under `[llm]` in corvid.toml, or "
            "pass `model=\"...\"` explicitly."
        )

    prompt_text = _render_template(meta["template"], meta["params"], args)
    tracing.record(
        "llm.request",
        prompt=prompt_name,
        model=chosen,
        prompt_chars=len(prompt_text),
    )
    adapter = _pick_adapter(chosen)
    result = await adapter(chosen, prompt_text)
    tracing.record("llm.response", prompt=prompt_name, model=chosen)
    return result


def _render_template(template: str, param_names: list[str], args: list[Any]) -> str:
    """Replace `{param}` markers in the template with arg values."""
    out = template
    for name, value in zip(param_names, args):
        out = out.replace(f"{{{name}}}", _stringify(value))
    return out


def _stringify(v: Any) -> str:
    if isinstance(v, str):
        return v
    return repr(v)


# ----------------------------------------------------------------------
# Anthropic adapter (optional — requires `pip install corvid-runtime[anthropic]`).
# ----------------------------------------------------------------------


async def _anthropic_adapter(model: str, prompt_text: str) -> str:
    try:
        import anthropic  # type: ignore
    except ImportError as e:
        raise UnknownModel(
            "Anthropic adapter requested but `anthropic` not installed.\n"
            "  help: pip install 'corvid-runtime[anthropic]'"
        ) from e

    client = anthropic.AsyncAnthropic()
    message = await client.messages.create(
        model=model,
        max_tokens=4096,
        messages=[{"role": "user", "content": prompt_text}],
    )
    # Minimal text extraction. Richer structured-output handling lives in v0.2.
    parts = []
    for block in getattr(message, "content", []):
        text = getattr(block, "text", None)
        if text is not None:
            parts.append(text)
    return "".join(parts)


# Auto-register Anthropic for common model name prefixes. This is a no-op
# if the `anthropic` package isn't installed until the adapter is invoked.
register_adapter("claude-", _anthropic_adapter)
