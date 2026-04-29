# Gmail Connector

Phase 41 Gmail support starts with metadata read/search in mock and replay mode.
Real-provider mode is intentionally explicit and opt-in.

## Environment For Real Provider Mode

```sh
GOOGLE_CLIENT_ID=...
GOOGLE_CLIENT_SECRET=...
GOOGLE_OAUTH_REDIRECT_URI=http://localhost:8765/oauth/google/callback
CORVID_CONNECTOR_MODE=real
CORVID_CONNECTOR_TOKEN_STORE=target/connectors/tokens
```

Required Google scopes for 41C1:

```text
https://www.googleapis.com/auth/gmail.metadata
```

Write scopes such as `https://www.googleapis.com/auth/gmail.send` are not used
by 41C1. Sending lands in 41C2 and remains approval-gated.

## Mock Mode

Mock mode uses `GmailConnector::insert_mock` with operations:

- `search`
- `read_metadata`

The mock payload is the same typed `GmailMessageMetadata` JSON shape used by
replay fixtures.

## Replay Keys

- Search: `gmail:search:<user_id>:<stable-query>`
- Read metadata: `gmail:message:<user_id>:<message_id>`

Replay mode records read evidence and quarantines writes through the shared
connector runtime.
