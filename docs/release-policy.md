# Corvid Release Policy

This policy defines how Corvid ships nightly, beta, and stable releases. It is a maintainer contract: a release is not publishable unless the channel rules, SemVer rules, signed artifacts, migration notes, and claim review all pass.

## Channels

### Nightly

- Purpose: expose the latest merged work to early adopters and CI consumers.
- Version format: `0.0.0-nightly.YYYYMMDD+<short-sha>`.
- Cadence: at most once per UTC day from `main`.
- Stability: no source or binary compatibility promise beyond the generated upgrade report.
- Required artifacts: signed binary archive, `SHA256SUMS.txt`, release manifest, changelog fragment, and claim-audit report.
- Promotion rule: a nightly can become a beta candidate only when all Phase 43 launch gates are green or explicitly non-scope.

### Beta

- Purpose: validate the release candidate with real backend applications.
- Version format: `MAJOR.MINOR.PATCH-beta.N`.
- Cadence: weekly or on critical fix, never more than one beta per commit.
- Stability: source compatibility is expected inside one beta train; breaking changes require a new beta number and a migration note.
- Required artifacts: everything nightly ships, plus upgrade check output from the previous beta and stable baseline.
- Promotion rule: a beta can become stable only after beta feedback is closed as code, docs, tests, or explicit non-scope.

### Stable

- Purpose: production use by external Corvid developers.
- Version format: `MAJOR.MINOR.PATCH`.
- Cadence: explicit maintainer cut, not automatic.
- Stability: SemVer applies to the language surface, stdlib public surface, ABI attestation format, receipt verification behavior, deploy package shape, and connector manifest schema.
- Required artifacts: signed binaries, checksums, SBOM, release manifest, changelog, reproducible-build notes, migration guide, advisory contact, and final claim audit.
- Promotion rule: the stable tag is blocked if any launch claim lacks a runnable command, test, or explicit non-scope reason.

## SemVer Scope

SemVer covers these public contracts:

- Corvid source syntax accepted by the parser.
- Typechecker diagnostics that are documented as stable guarantees.
- Standard library modules documented in `docs/stdlib.md`.
- ABI descriptor and ABI attestation schema consumed by `corvid receipt verify-abi`.
- Trace, receipt, migration-state, and connector-manifest schemas.
- CLI commands documented for production flows: `build`, `migrate`, `deploy`, `release`, `upgrade`, `ops`, and `claim`.

Patch releases may add diagnostics, tighten security checks, and fix runtime behavior when the documented contract already required that behavior. Minor releases may add syntax, stdlib APIs, connector fields, and deployment targets. Major releases may remove or change public contracts only with an upgrade tool and migration guide.

## Stability Classes

- `stable`: supported for all patch and minor releases in the same major line.
- `beta`: may change during beta, but every change must be represented in the upgrade report.
- `experimental`: available only behind an explicit flag or preview namespace; never used in launch claims.
- `internal`: not a public contract and not covered by SemVer.

Every public feature must declare one of these classes in its docs or generated manifest before a stable release.

## Breaking Change Rules

A change is breaking when it removes syntax, changes a public type, weakens a guarantee, changes a serialized schema without migration tooling, changes a stable CLI flag, or changes deploy artifact layout in a way that invalidates existing automation.

Breaking changes require:

- a migration note,
- an upgrade check rule,
- at least one accepted fixture and one rejected fixture when source syntax changes,
- a changelog entry,
- maintainer approval in the release checklist.

## Release Blockers

A release is blocked when any of these are true:

- signed artifact generation fails,
- checksums or SBOM are missing,
- `corvid claim audit` reports an aspirational launch claim,
- `corvid upgrade --check` reports an unhandled weakening,
- generated docs drift from the guarantee registry,
- migration execution or drift detection fails,
- a security advisory marked release-blocking is open,
- beta feedback required for stable remains untriaged.

## Key Rotation

Release keys are rotated by publishing a signed key-rotation note in the previous key and the new key. A stable release must name the release key id in the release manifest. Revoked keys may verify historical artifacts but may not sign new releases.

## Maintainer Signoff

Stable release signoff requires:

- one maintainer responsible for release mechanics,
- one maintainer responsible for security/advisory review,
- one maintainer responsible for claim audit,
- one maintainer responsible for compatibility and migration review.

The same person may hold more than one role, but each role must be recorded in the release manifest.
