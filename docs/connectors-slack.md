# Slack Connector

The Slack connector exposes workspace-scoped channel, DM, and thread reads.
Message text is represented by fingerprints in trace-oriented metadata; raw
message text must be redacted before observability export.

## Environment For Real Provider Mode

```sh
SLACK_CLIENT_ID=...
SLACK_CLIENT_SECRET=...
SLACK_SIGNING_SECRET=...
CORVID_CONNECTOR_MODE=real
CORVID_CONNECTOR_TOKEN_STORE=target/connectors/tokens
```

Read scopes for 41F1:

```text
channels:history
im:history
```

## Mock Mode

Mock operations:

- `channel_read`
- `dm_read`
- `thread_read`

## Replay Keys

- Channel read: `slack:channel_read:<workspace_id>:<channel_id>:<user_id>:<since_ms>`
- DM read: `slack:dm_read:<workspace_id>:<channel_id>:<user_id>:<since_ms>`
- Thread read: `slack:thread:<workspace_id>:<channel_id>:<thread_ts>:<user_id>`

Write scopes:

```text
chat:write
```

Draft and send operations require approval IDs and preserve the approval ID in
the write receipt. Replay mode quarantines writes.

Write operations:

- `draft`
- `send`

Write replay keys:

- Draft: `slack:draft:<workspace_id>:<channel_id>:<user_id>`
- Send: `slack:send:<workspace_id>:<channel_id>:<draft_id>`
