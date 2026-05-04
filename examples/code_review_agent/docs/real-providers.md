# Code Review Agent Real Providers

Real-provider mode is opt-in. Default builds, tests, evals, and CI use mock
GitHub and mock LLM responses.

## Enable Real Mode

Set all required variables before running the app:

```sh
CORVID_RUN_REAL=1
GITHUB_TOKEN=ghp_example_placeholder
GITHUB_REPOSITORY=Corvid-lang/Corvid-lang
GITHUB_PULL_REQUEST=418
OPENAI_API_KEY=sk_example_placeholder
OPENAI_MODEL=gpt-4o-mini
```

Use provider-specific secret storage for the token values. Do not place live
tokens in seed files, trace fixtures, shell history, or README examples.

## Providers

GitHub:

- Minimum target: GitHub REST API v3.
- Required scope for read-only review runs: pull request read access.
- Required scope for approved comment posting: pull request write access.
- The write path remains behind `approve PostReviewComment(...)`.

LLM:

- Default live adapter: OpenAI chat-compatible structured output.
- Required variable: `OPENAI_API_KEY`.
- Optional variable: `OPENAI_MODEL`; defaults should match the repository's
  current LLM adapter configuration when unset.

## Mock, Replay, Real

All modes share the `PullRequestDiff`, `ReviewComment`, `ReviewReceipt`, and
`ReviewSession` surfaces in `src/main.cor`.

- Mock mode reads deterministic `CORVID_TEST_MOCK_TOOLS` and
  `CORVID_TEST_MOCK_LLM_REPLIES` payloads.
- Replay mode substitutes committed responses from
  `traces/code_review_agent_review_session.jsonl` or
  `seed/traces/code_review_agent_review_session.jsonl`.
- Real mode may call GitHub and the configured LLM only when
  `CORVID_RUN_REAL=1` is set.

## Redaction

Replay traces must use redacted placeholders for repository SHAs and must never
contain GitHub tokens, API keys, webhook secrets, private emails, or private
repository URLs. Before committing any new trace, run the repository redaction
pipeline and verify:

```sh
rg -n "[s]k-|g[h]o_|w[h]sec_" examples/code_review_agent
```
