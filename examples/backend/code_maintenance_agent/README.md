# Code Review and Maintenance Agent Backend

42G1 ships repo ingestion, issue triage, and CI-aware risk labeling in mock
mode.

## Routes

- `GET /config`
- `GET /repos/ingest/mock`
- `GET /issues/triage/mock`
- `GET /writes/plan/mock`
- `POST /comments/post`
- `POST /patches/propose`

42G2 adds approval-gated write actions for review comments and patch proposals.
The committed write plan uses fingerprints, not raw patch content.
