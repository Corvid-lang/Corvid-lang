# Finance Operations Agent Backend

42E1 ships the read-only half of the Finance Operations reference app. It
aggregates account, budget, subscription, reminder, and anomaly data in mock
mode without making payments or regulated advice claims.

## Routes

- `GET /config`
- `GET /readonly/snapshot/mock`

## Posture

- Read-only by default.
- Mock data uses fingerprints for sensitive financial explanations.
- `regulated_advice` is false and remains false until the explicit non-scope and
  approval audit slice is complete.
