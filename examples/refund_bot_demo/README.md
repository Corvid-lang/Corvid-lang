# refund_bot demo

A self-contained Corvid program with mocked tools and a fake LLM, so you
can run it end-to-end without an API key.

## Run

```bash
cd examples/refund_bot_demo
corvid build src/refund_bot.cor
python3 tools.py
```

Expected output:

```
refund_bot decided: should_refund=True reason='user reported legitimate complaint'
```

## What's going on

- `src/refund_bot.cor` — the Corvid source.
- `tools.py` — mock implementations of `get_order` and `issue_refund`, a fake LLM adapter, and the `main` entry point.
- `corvid build ...` generates `target/py/refund_bot.py`.
- `tools.py` imports the generated module and invokes the agent.

Delete the `approve IssueRefund(...)` line in `src/refund_bot.cor` and run
`corvid check` — it won't compile.
