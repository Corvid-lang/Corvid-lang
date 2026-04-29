# Personal Executive Agent Security Model

- Email, calendar, task, chat, and file writes require approvals.
- Demo mode uses mock connectors and quarantines writes.
- Trace fixtures store fingerprints, approval IDs, replay keys, and redaction
  hashes, not raw message bodies.
- Provider tokens stay in connector token state.
