# Code Review and Maintenance Agent Backend

42G1 ships repo ingestion, issue triage, and CI-aware risk labeling in mock
mode.

## Routes

- `GET /config`
- `GET /repos/ingest/mock`
- `GET /issues/triage/mock`

The app reads repository, issue, and CI metadata only. Write actions are added
behind approval gates in 42G2.
