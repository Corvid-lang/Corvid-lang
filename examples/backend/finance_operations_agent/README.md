# Finance Operations Agent Backend

42E1 ships the read-only half of the Finance Operations reference app. It
aggregates account, budget, subscription, reminder, and anomaly data in mock
mode without making payments or regulated advice claims.

## Routes

- `GET /config`
- `GET /readonly/snapshot/mock`
- `GET /payments/intents/mock`
- `POST /payments/intents/submit`

## Posture

- Read-only by default.
- Mock data uses fingerprints for sensitive financial explanations.
- `regulated_advice` is false and remains false until the explicit non-scope and
  approval audit slice is complete.

## Payment Intent Boundary

42E2 adds payment intents, not autonomous payments:

- `POST /payments/intents/submit` is a dangerous write and must pass through
  `approve SubmitPaymentIntent(...)`.
- `GET /payments/intents/mock` exposes the intent plus redacted audit record.
- `regulated_advice` stays false, and all outputs are operational summaries, not
  investment, tax, credit, or legal advice.
