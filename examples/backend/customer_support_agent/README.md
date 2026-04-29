# Customer Support Operations Agent Backend

42F1 ships ticket triage and policy-grounded draft replies in mock mode.

## Routes

- `GET /config`
- `GET /tickets/triage/mock`
- `GET /replies/draft/mock`

Draft replies carry policy citations and remain approval-pending until the 42F2
write/approval slice.
