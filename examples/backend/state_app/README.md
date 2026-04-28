# Backend State App

This example is the Phase 37 durable-state reference shape for a Corvid backend.
It models the minimum state surface a production Personal Executive Agent needs:
users, tasks, approvals, traces, connector tokens, and durable agent checkpoints.

## Migrations

Run migration checks in CI before deploy:

```powershell
cargo run -p corvid-cli -- migrate status --dir examples/backend/state_app/migrations --state target/state-app-migrations.json --dry-run
```

Apply migrations only from reviewed, checked-in SQL files:

```powershell
cargo run -p corvid-cli -- migrate up --dir examples/backend/state_app/migrations --state target/state-app-migrations.json
```

If drift is reported, stop the deploy. Do not edit an applied migration in place;
add a new forward migration and keep the old checksum history intact.

## Backup And Rollback

Back up the database before every migration batch and retain the migration state
file with the backup artifact. Rollback is an operator action: restore the
database backup and matching migration state file together, then run a dry-run
status check before serving traffic.

## Redaction

Connector token rows store only encrypted token material plus a ciphertext hash,
key id, and provider/account metadata. Application traces, audit rows, migration
reports, and replay summaries must never store raw access tokens, refresh tokens,
or encryption keys.

## Operator Checks

- `corvid migrate status --dry-run` must report zero drift before deploy.
- Token encryption key shape is validated by `corvid doctor`.
- Audit writes that represent dangerous AI actions must carry an approval state.
- Replay summaries should include query fingerprints and row counts, not raw row
  values or secrets.
