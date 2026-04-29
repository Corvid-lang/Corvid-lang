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

Write scopes land in 41E2 and require approval.

## Mock Mode

Mock operations:

- `availability`
- `events`

Mock payloads use `CalendarAvailabilitySlot` and `CalendarEvent`.

## Replay Keys

- Availability: `calendar:availability:<user_id>:<start_ms>:<end_ms>:<duration_ms>`
- Events: `calendar:events:<user_id>:<calendar_id>:<start_ms>:<end_ms>`
