# Code Review Agent

Code review agent that reads a pull request diff, asks a structured review
prompt for a checklist-backed finding, and keeps GitHub comment posting behind
an explicit approval.

## Setup

From this directory:

```sh
set CORVID_TEST_MOCK_TOOLS={"fetch_pull_request_diff":{"repo":"Corvid-lang/Corvid-lang","number":418,"base_sha":"base_redacted_20260504","head_sha":"head_redacted_20260504","diff":"approval bypass diff"}}
set CORVID_TEST_MOCK_LLM=1
set CORVID_TEST_MOCK_LLM_REPLIES={"draft_review_comment":{"path":"examples/refund_bot/src/main.cor","line":43,"severity":"high","checklist_id":"approval-boundary","body":"shortcut_refund calls issue_refund without an approval boundary; require approve IssueRefund before the tool call."}}
cargo run -q -p corvid-cli -- build
cargo run -q -p corvid-cli -- run
```

On macOS or Linux, use `export` instead of `set`.

Expected output includes:

```text
ReviewSession(... finding_count: 1 ... checklist_id: "approval-boundary" ... posted: false ...)
```

## What It Shows

- `fetch_pull_request_diff` and `post_review_comment` model the GitHub
  connector boundary with one typed surface.
- `draft_review_comment` returns a structured `ReviewComment`, not free-form
  prose that downstream code has to parse.
- The prompt tells the model that diff contents are untrusted input, and the
  unit test covers an injected diff instruction.
- `post_review_comment` is dangerous and cannot be called without
  `approve PostReviewComment(...)`.
- Replay covers the full read plus structured-review path deterministically.

## Verify

From the repository root:

```sh
cargo run -q -p corvid-cli -- test examples/code_review_agent/tests/unit.cor
cargo run -q -p corvid-cli -- test examples/code_review_agent/tests/integration.cor
cargo run -q -p corvid-cli -- eval examples/code_review_agent/evals/code_review_agent.cor
cargo run -q -p corvid-cli -- replay examples/code_review_agent/traces/code_review_agent_review_session.jsonl
```

Set the mock env vars from the setup section before running tests or evals.
Replay does not need mock env vars because it substitutes the committed trace
responses.

## How To Modify

Add new review scenarios under `seed/`, update the mock tool and LLM queues,
then update tests, eval assertions, and replay fixtures together. Keep any
GitHub write behind an explicit `approve` statement and add a compiler
rejection test for the unapproved variant.
