# Code Review Agent Runbook

## Deploy

1. Start from a clean clone on `main`.
2. Run the mock verification loop:

   ```sh
   cargo run -q -p corvid-cli -- test examples/code_review_agent/tests/unit.cor
   cargo run -q -p corvid-cli -- test examples/code_review_agent/tests/integration.cor
   cargo run -q -p corvid-cli -- test examples/code_review_agent/tests/replay_invariant.cor
   cargo run -q -p corvid-cli -- eval examples/code_review_agent/evals/code_review_agent.cor
   cargo run -q -p corvid-cli -- replay examples/code_review_agent/seed/traces/code_review_agent_review_session.jsonl
   ```

3. Configure real mode only in the deployment secret store:

   ```sh
   CORVID_RUN_REAL=1
   GITHUB_TOKEN=<github-token-ref>
   GITHUB_REPOSITORY=<owner/repo>
   GITHUB_PULL_REQUEST=<number>
   OPENAI_API_KEY=<openai-key-ref>
   ```

4. Run the app in read-only review mode first. Do not enable approved comment
   posting until the operator has reviewed the generated `ReviewComment`.

## Observe

- Confirm each run emits one `ReviewSession` with `posted: false` unless the
  operator explicitly chose the approved write path.
- Track GitHub read status, LLM structured-output parse status, and approval
  prompts separately.
- Review trace output for redacted SHAs and absence of credential-shaped
  values before sharing or committing traces.
- Watch for drift between mock, replay, and real outputs by rerunning
  `tests/replay_invariant.cor` after fixture or provider updates.

## Rollback

1. Unset `CORVID_RUN_REAL` or remove the deployment secret binding.
2. Stop any job invoking the real GitHub write path.
3. Revert to replay mode with
   `seed/traces/code_review_agent_review_session.jsonl`.
4. If a bad comment was posted, remove or resolve the GitHub review comment and
   keep the redacted incident trace for follow-up.

## Incident Response

Credential exposure:

- Revoke the GitHub or LLM key immediately.
- Search source, traces, seed data, logs, and CI artifacts for credential-shaped
  values.
- Replace committed fixtures only with redacted traces.

Unapproved comment posting:

- Treat any successful `post_review_comment` call without an operator approval
  record as a release-blocking incident.
- Preserve the trace and approval log.
- Add or tighten an adversarial fixture before re-enabling real write mode.

Prompt injection or bad review output:

- Save the diff, prompt inputs, structured output, and trace in redacted form.
- Add a focused eval or adversarial fixture for the new injection surface.
- Keep real posting disabled until the mock and replay checks pass.

Provider outage:

- Fall back to replay fixtures for regression checks.
- Do not switch providers without updating `real-providers.md`, seed data, and
  the replay invariant if the typed surface changes.

## Release Checklist

- Mock tests pass.
- Eval passes.
- Original and seed replay fixtures pass.
- Adversarial fixtures are checked by `demo_project_defaults`.
- Credential scan returns no matches for committed code review app files.
