# Customer Support Operations Agent Backend

42F1 ships ticket triage and policy-grounded draft replies in mock mode.

## Routes

- `GET /config`
- `GET /tickets/triage/mock`
- `GET /replies/draft/mock`
- `GET /sla/jobs/mock`
- `GET /eval/dashboard/mock`
- `POST /replies/send`
- `POST /refunds/issue`

Draft replies carry policy citations. 42F2 adds approval-gated reply sends,
approval-gated refunds, replayable SLA jobs, and an eval dashboard fixture.
