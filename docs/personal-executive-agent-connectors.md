# Personal Executive Agent Connector Wiring

The Personal Executive Agent must use Corvid-owned connectors for every external
system. It should not call provider SDKs directly from app code.

## Connector Map

| Product Area | Connector | Read Operations | Write Operations |
| --- | --- | --- | --- |
| Inbox triage | Gmail | `search`, `read_metadata` | `draft`, `send` |
| Calendar scheduling | Calendar | `availability`, `events` | `create`, `update`, `cancel` |
| Task extraction | Tasks | `linear_search`, `github_search` | `linear_write`, `github_write` |
| Chat follow-up | Slack | `channel_read`, `dm_read`, `thread_read` | `draft`, `send` |
| Personal knowledge | Files | `index`, `read` | `write`, `delete` |

## Required Approval Boundaries

These actions require approval IDs before execution:

- Gmail draft/send.
- Calendar create/update/cancel, especially with external attendees.
- Slack draft/send.
- Linear/GitHub create/update/comment.
- Local file create/update/delete.

Replay mode must quarantine all of the above writes.

## Mock-First Test Plan

The executive agent test suite should run in mock connector mode by default:

```text
GmailConnector::search_metadata -> inbox triage
GmailConnector::draft_reply -> approval queue
CalendarConnector::availability -> scheduling options
CalendarConnector::create_event -> approval queue
TaskConnector::write_linear/write_github -> approval queue
SlackConnector::draft_message -> approval queue
FileConnector::index/read -> meeting prep context
```

The proof test in 41J2 must show email, calendar, tasks, chat, and files all run
through connector mocks and that every dangerous write has an approval ID.

## Trace Requirements

Every connector call must preserve:

- connector name
- operation
- tenant/actor
- scope
- effect IDs
- data classes
- approval ID for writes
- replay key

These fields are already emitted by `ConnectorTraceEvent`.
