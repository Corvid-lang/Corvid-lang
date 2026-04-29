# Calendar Connector

The calendar connector provides provider-neutral availability and event read
paths. Provider-specific backing clients can map Google Calendar, Microsoft
Graph calendar, or CalDAV into this typed surface.

## Environment For Real Provider Mode

```sh
CORVID_CALENDAR_PROVIDER=google|ms365|caldav
CORVID_CONNECTOR_MODE=real
CORVID_CONNECTOR_TOKEN_STORE=target/connectors/tokens
```

Read scopes for 41E1:

```text
calendar.read
```

Write scopes:

```text
calendar.write
```

Event create, update, cancel, and external invites require an approval ID.
Replay mode quarantines these writes.

## Mock Mode

Mock operations:

- `availability`
- `events`
- `create`
- `update`
- `cancel`

Mock payloads use `CalendarAvailabilitySlot` and `CalendarEvent`.

## Replay Keys

- Availability: `calendar:availability:<user_id>:<start_ms>:<end_ms>:<duration_ms>`
- Events: `calendar:events:<user_id>:<calendar_id>:<start_ms>:<end_ms>`
- Create: `calendar:create:<user_id>:<calendar_id>:<start_ms>`
- Update: `calendar:update:<user_id>:<calendar_id>:<start_ms>`
- Cancel: `calendar:cancel:<user_id>:<calendar_id>:<event_id>`
