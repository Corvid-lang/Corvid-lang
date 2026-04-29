# Gmail Connector

Phase 41 Gmail support starts with metadata read/search and approval-gated
draft/send in mock and replay mode. Real-provider mode is intentionally explicit
and opt-in.

## Environment For Real Provider Mode

```sh
GOOGLE_CLIENT_ID=...
GOOGLE_CLIENT_SECRET=...
GOOGLE_OAUTH_REDIRECT_URI=http://localhost:8765/oauth/google/callback
CORVID_CONNECTOR_MODE=real
CORVID_CONNECTOR_TOKEN_STORE=target/connectors/tokens
```

Required Google scopes:

```text
https://www.googleapis.com/auth/gmail.metadata
https://www.googleapis.com/auth/gmail.compose
https://www.googleapis.com/auth/gmail.send
```

Draft and send operations require an approval ID. Replay mode quarantines writes.

## Mock Mode

Mock mode uses `GmailConnector::insert_mock` with operations:

- `search`
- `read_metadata`
- `draft`
- `send`

The mock payload is the same typed `GmailMessageMetadata` JSON shape used by
replay fixtures.

## Replay Keys

- Search: `gmail:search:<user_id>:<stable-query>`
- Read metadata: `gmail:message:<user_id>:<message_id>`
- Draft: `gmail:draft:<user_id>:<stable-subject>`
- Send: `gmail:send:<user_id>:<draft_id>`

Replay mode records read evidence and quarantines writes through the shared
connector runtime.
