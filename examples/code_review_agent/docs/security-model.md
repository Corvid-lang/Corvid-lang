# Code Review Agent Security Model

This document extends the canonical Corvid security model in
[`docs/security-model.md`](../../../docs/security-model.md). It does not add
new global guarantees; it maps the code review app's threat surface to existing
Corvid guarantees and local tests.

## Trust Boundary

```text
pull request id -> fetch_pull_request_diff tool -> PullRequestDiff
      |                                         |
      |                                         v
      |                             draft_review_comment prompt
      |                                         |
      v                                         v
operator approval ---- approve PostReviewComment(repo, number, comment)
                                                |
                                                v
                              post_review_comment dangerous tool
```

The app-specific trusted computing base is:

- Corvid parser, resolver, typechecker, approval checker, runtime, and replay
  engine as defined in the canonical security model.
- The `fetch_pull_request_diff`, `draft_review_comment`, and
  `post_review_comment` declarations in `src/main.cor`.
- The `PostReviewComment` approval site before the dangerous GitHub write.
- The shared `PullRequestDiff`, `ReviewComment`, `ReviewReceipt`, and
  `ReviewSession` surfaces used by mock, replay, and real provider modes.
- The prompt instruction that treats diff contents as untrusted input.
- Operator-controlled environment variables for real mode.

## Protected Assets

- GitHub write authority: posting review comments requires an explicit approval
  token.
- Repository diff contents: prompt-like text inside a diff must stay data, not
  authority to post or suppress comments.
- GitHub and LLM credentials: secrets must never enter source, seed fixtures,
  traces, or CI logs.
- Replay fixtures: committed traces must remain deterministic and redacted.
- Review output shape: `ReviewComment` and `ReviewSession` fields must remain
  stable across mock, replay, and real modes.

## Named Threats

| Threat | Defense | Test |
| --- | --- | --- |
| `post_comment_without_approval` | A direct call to `post_review_comment` without `approve PostReviewComment(...)` is rejected with `approval.dangerous_call_requires_token`. | `tests/adversarial/post_comment_without_approval.cor` |
| `prompt_injection_diff` | A diff-supplied instruction cannot authorize the dangerous GitHub write. | `tests/adversarial/prompt_injection_diff.cor` |
| `github_token_trace_leak` | A write path that would place token-like material into a review receipt still cannot call the dangerous tool without approval. | `tests/adversarial/github_token_trace_leak.cor` |
| `supply_chain_diff_source` | A comment derived from an untrusted fork or repository source cannot be posted without approval. | `tests/adversarial/supply_chain_diff_source.cor` |

`crates/corvid-cli/tests/demo_project_defaults.rs` typechecks each adversarial
fixture and asserts the registered `approval.dangerous_call_requires_token`
guarantee id.

## Replay Invariant

`tests/replay_invariant.cor` asserts that mock, replay, and real entrypoints
return the same `ReviewSession` fields for the deterministic review seed path.
Mode selection is host configuration, not part of the typed result surface.
Provider latency, token counts, raw provider payloads, and request ids belong in
redacted trace metadata or provider logs.

## Non-Goals

- This demo does not prove that an LLM finds every real code review issue. It
  demonstrates structured review output and the approval boundary for GitHub
  writes.
- This demo does not prove semantic prompt-injection immunity. The prompt and
  tests preserve the local policy for the committed seed scenario; operators
  still need eval coverage for new review policies and diff shapes.
- This demo does not prove GitHub token scope minimization beyond documenting
  required scopes and rejecting unapproved write paths.
- This demo does not post comments in default CI. Real provider posting remains
  opt-in behind `CORVID_RUN_REAL=1` and the approval gate.
