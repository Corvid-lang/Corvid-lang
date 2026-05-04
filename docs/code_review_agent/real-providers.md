# Code Review Agent Real Providers

Real provider mode is opt-in. CI and normal tests use deterministic mock
GitHub and mock LLM responses.

## Environment

Set the global opt-in first:

```sh
CORVID_RUN_REAL=1
```

Then configure GitHub and one LLM provider:

```sh
GITHUB_TOKEN=<redacted-github-token>
GITHUB_OWNER=Corvid-lang
GITHUB_REPO=Corvid-lang
GITHUB_PULL_NUMBER=418
OPENAI_API_KEY=<redacted-openai-key>
ANTHROPIC_API_KEY=<redacted-anthropic-key>
OLLAMA_BASE_URL=http://localhost:11434
```

Use a GitHub token scoped to read pull request diffs for review-only runs. Add
comment write scope only for flows that call `approved_post_review`.

## Default Mock Mode

The default path does not require real credentials:

```sh
CORVID_TEST_MOCK_TOOLS={"fetch_pull_request_diff":{"repo":"Corvid-lang/Corvid-lang","number":418,"base_sha":"base_redacted_20260504","head_sha":"head_redacted_20260504","diff":"approval bypass diff"}}
CORVID_TEST_MOCK_LLM=1
CORVID_TEST_MOCK_LLM_REPLIES={"draft_review_comment":{"path":"examples/refund_bot/src/main.cor","line":43,"severity":"high","checklist_id":"approval-boundary","body":"shortcut_refund calls issue_refund without an approval boundary; require approve IssueRefund before the tool call."}}
```
