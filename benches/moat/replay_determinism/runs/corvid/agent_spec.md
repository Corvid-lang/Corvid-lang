# Reference agent — refund_bot

This is the contract a Python or TypeScript bounty submission must
reproduce. Behavior is fully specified by the Corvid source plus the
mocked tool / LLM responses below; nothing is left to ambient
configuration.

## Inputs

A single ticket struct, identical across every run:

```json
{
  "order_id": "ord_42",
  "user_id":  "user_1",
  "message":  "package arrived broken — please refund"
}
```

## Tools (mocked)

```text
get_order(id: string) -> { id, amount: f64, user_id: string }
  Always returns: { id: "ord_42", amount: 49.99, user_id: "user_1" }

issue_refund(id: string, amount: f64) -> { refund_id: string, amount: f64 }
  Always returns: { refund_id: "rf_<id>", amount: <amount> }
```

## LLM (mocked)

The decide_refund prompt is invoked once with rendered text:

```
Decide whether this ticket deserves a refund. Consider the order amount,
the user's complaint, and fairness.
```

(Plus the ticket and order as context args.)

Mock returns canned:

```json
{ "should_refund": true, "reason": "user reported legitimate complaint" }
```

## Approval

The agent requests an `IssueRefund` approval with args `["ord_42",
49.99]`. Approver always returns `approved: true`.

## Expected final return

```json
{ "should_refund": true, "reason": "user reported legitimate complaint" }
```

## Expected causal trace (Corvid order)

```
schema_header
seed_read           (rollout_default_seed)
run_started         (agent=refund_bot, args=[ticket])
tool_call           (get_order, ["ord_42"])
tool_result         (get_order, {id, amount, user_id})
llm_call            (decide_refund, rendered, [ticket, order])
llm_result          (decide_refund, {should_refund, reason})
approval_request    (IssueRefund, ["ord_42", 49.99])
approval_response   (IssueRefund, approved=true)
tool_call           (issue_refund, ["ord_42", 49.99])
tool_result         (issue_refund, {refund_id: "rf_ord_42", amount: 49.99})
run_completed       (ok=true, result={should_refund, reason})
```

A submission's stack-native trace need not have the same event names,
but it must capture each of these causal events distinguishably so an
auditor can reconstruct what happened.

## Deterministic-side requirements

- The mocked LLM must not depend on wall-clock or random state.
- The tools must not depend on wall-clock or random state.
- The submission's normalization rules must be documented in
  `runs/<stack>/README.md` and applied uniformly across the N runs.
- Library versions must be pinned in
  `runs/<stack>/requirements.txt` (Python) or
  `runs/<stack>/package.json` (TypeScript).
