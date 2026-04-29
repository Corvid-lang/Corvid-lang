# Microsoft 365 Connector

Phase 41 Microsoft 365 support starts with Microsoft Graph mail search and
calendar event reads. Tenant-aware auth is represented in connector auth state
through `tenant_id` and in the token scope set.

## Environment For Real Provider Mode

```sh
MS365_TENANT_ID=...
MS365_CLIENT_ID=...
MS365_CLIENT_SECRET=...
MS365_OAUTH_REDIRECT_URI=http://localhost:8765/oauth/ms365/callback
CORVID_CONNECTOR_MODE=real
CORVID_CONNECTOR_TOKEN_STORE=target/connectors/tokens
```

Required Graph delegated scopes for 41D1:

```text
Mail.Read
Calendars.Read
```

## Tenant-Aware Refresh

Refresh token state is tenant-bound. `ConnectorRefreshTokenState::refresh`
rejects cross-tenant refresh attempts and revoked refresh tokens before issuing a
new access-token state.

## Mock Mode

Mock operations:

- `mail_search`
- `calendar_events`

The mock payloads use the typed `Ms365MailMessage` and `Ms365CalendarEvent`
JSON shapes.

## Replay Keys

- Mail search: `ms365:mail:<user_id>:<stable-query>`
- Calendar events: `ms365:calendar:<user_id>:<start_ms>:<end_ms>`

Write operations are not part of 41D1. Later Graph writes remain approval-gated
and replay-quarantined through the shared connector runtime.
