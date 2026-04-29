# Task Connectors

The task connector covers Linear and GitHub issues through one typed task issue
surface. Phase 41G1 ships read/search in mock and replay mode.

## Environment For Real Provider Mode

```sh
LINEAR_API_KEY=...
GITHUB_TOKEN=...
CORVID_CONNECTOR_MODE=real
CORVID_CONNECTOR_TOKEN_STORE=target/connectors/tokens
```

Read scopes:

```text
linear:read
github:issues:read
```

## Mock Mode

Mock operations:

- `linear_search`
- `github_search`

## Replay Keys

- Linear search: `tasks:linear:<workspace_id>:<stable-query>`
- GitHub search: `tasks:github:<owner>/<repo>:<stable-query>`

Write operations land in 41G2 and require approval.
