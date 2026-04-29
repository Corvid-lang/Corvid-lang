# Corvid Maintainer Runbooks

These runbooks define the minimum maintainer process for a production release. They are intentionally operational: every section maps to a command, artifact, or explicit decision record.

## Release Checklist

Before cutting nightly, beta, or stable:

- run `cargo test --workspace` or the documented CI equivalent,
- run `cargo run -q -p corvid-cli -- contract regen-doc docs/core-semantics.md` and confirm no drift,
- run `corvid upgrade check . --json`,
- run `corvid claim audit` for launch-facing claims,
- run `corvid release <channel> <version> --out target/release/<channel>`,
- verify `SHA256SUMS.txt`,
- verify `release-attestation.dsse.json`,
- attach changelog, SBOM, checksums, release manifest, and reproducible-build notes.

Stable releases also require the beta-feedback closure report, migration guide, advisory-contact check, and final launch rehearsal.

## Security Advisory Process

1. Triage incoming reports within two business days.
2. Assign severity: critical, high, medium, low, or informational.
3. Mark release-blocking advisories in the release issue.
4. Patch privately when exploitability is credible.
5. Add regression tests before disclosure unless doing so would reveal an active exploit.
6. Publish advisory with affected versions, fixed versions, workaround, and verification command.
7. Rotate release keys if signing material may be affected.

Security contact and incident-response ownership must be present in the stable release manifest.

## Compatibility Policy

Compatibility follows `docs/release-policy.md`.

- Source syntax, stdlib APIs, ABI attestation, receipt verification, trace schemas, migration state, connector manifests, and stable CLI flags are public contracts.
- Breaking changes need an upgrade rule, migration note, changelog entry, and maintainer signoff.
- Patch releases may tighten security behavior when the documented contract already required it.
- Experimental features cannot appear in launch claims.

## CI Gates

The release branch must pass:

- parser/typechecker/unit suites,
- native parity and binary suites,
- byte-fuzz and source-bypass corpora,
- ABI verification tests,
- guarantee registry validation,
- docs/core-semantics drift check,
- reference app checks,
- deploy package and release artifact tests,
- upgrade migrator tests.

A skipped gate needs an issue link and explicit non-scope note in the release manifest.

## Benchmark Reproduction

Benchmark claims must include:

- exact command,
- repository commit,
- machine shape,
- input corpus,
- expected output hash or summary,
- comparison target and version.

For clone-to-production-shaped-deploy, maintainers reproduce `benches/comparisons/clone_to_deploy.md` and attach the generated report before using the benchmark in launch material.

## Claim Review

Every public claim must be classified as:

- runnable artifact,
- documented guarantee,
- benchmark result,
- explicit non-scope,
- removed.

Claim review uses the guarantee registry as source of truth. A launch claim is blocked if it says Corvid enforces behavior that the registry marks `out_of_scope`, or if it has no runnable command, test, or signed artifact.

## Rollback

Rollback requires:

- revoke or hide the broken release artifact,
- publish a rollback note,
- identify affected channel and version,
- keep checksums and attestations for forensic verification,
- open follow-up work for any failed release gate.
