# Code Review Agent Benchmark Notes

This demo is performance-relevant at the long agent-loop boundary: one GitHub
diff read, one structured LLM review call, and an optional approval-gated
GitHub comment write.

Local smoke measurements should focus on:

- `corvid run` with `CORVID_TEST_MOCK_TOOLS` and `CORVID_TEST_MOCK_LLM_REPLIES`
- `corvid test examples/code_review_agent/tests/unit.cor`
- `corvid eval examples/code_review_agent/evals/code_review_agent.cor`
- `corvid replay examples/code_review_agent/traces/code_review_agent_review_session.jsonl`

The committed mock path is deterministic and has no provider wait. Real
provider latency depends on GitHub API round trips, selected LLM model latency,
diff size, and any human approval wait before posting a comment.

For a LangChain comparison, use the same sample diff and checklist with one
GitHub read, one LLM structured-output call, and one approval-controlled
comment write. Record the wall time and whether the framework enforces the
write approval boundary before the comment tool executes.
