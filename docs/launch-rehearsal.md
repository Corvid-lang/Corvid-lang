# Launch Rehearsal

The launch rehearsal proves the release package can be installed, explained, rolled back, and demonstrated without inventing claims during launch week.

## Smoke Commands

```bash
corvid check examples/backend/personal_executive_agent/src/main.cor
corvid audit examples/backend/personal_executive_agent/src/main.cor --json
corvid deploy package examples/backend/personal_executive_agent --out target/pea-package
corvid release beta 1.0.0-beta.1 --out target/release/beta
corvid claim audit --json
```

## Generated Release Files

The release directory must contain:

- release binary,
- `SHA256SUMS.txt`,
- `release-manifest.json`,
- `release-attestation.dsse.json`,
- `CHANGELOG.md`,
- `install.sh`,
- `install.ps1`,
- `REPRODUCIBLE.md`,
- `DEMO.md`,
- `INCIDENT_CONTACTS.md`,
- `ROLLBACK.md`.

## Incident Contacts

Stable launch requires named owners for release mechanics, security response, claim audit, and rollback. Placeholder contacts block stable.

## Rollback

Rollback preserves the broken artifact for investigation, stops promotion, publishes affected versions and workaround, and cuts a fixed nightly or beta only after release, upgrade, deploy, and claim-audit checks rerun.
