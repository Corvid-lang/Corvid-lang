# Refund Bot

Approval-gated money movement in a minimal Corvid project.

## Setup

From this directory:

```sh
cargo run -q -p corvid-cli -- build
cargo run -q -p corvid-cli -- run
```

Expected output includes:

```text
DemoStatus(contract: "approval-gated refund", app: "refund_bot")
```

## What It Shows

- `issue_refund` is a dangerous tool because it uses the `transfer_money`
  effect.
- `approve_refund` can call `issue_refund` only after an explicit approval
  statement.
- A variant that calls `issue_refund` without approval is rejected by
  `corvid check`.
- The committed trace fixture replays the one-command demo output
  deterministically.

## Verify

From the repository root:

```sh
cargo run -q -p corvid-cli -- test examples/refund_bot/tests/unit.cor
cargo run -q -p corvid-cli -- test examples/refund_bot/tests/integration.cor
cargo run -q -p corvid-cli -- eval examples/refund_bot/evals/refund_bot.cor
cargo run -q -p corvid-cli -- replay examples/refund_bot/traces/refund_bot_approval_gate.jsonl
```

## How To Modify

Change the fields on `RefundRequest` or the return shape of `RefundResponse`
in `src/main.cor`, then update the tests and replay fixture together. If a new
money-moving path is added, keep it behind an explicit approval statement and
add a negative test proving the unapproved call is rejected.
