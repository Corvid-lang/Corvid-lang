# Phase 43 Market Readiness

Phase 43 is the launch readiness phase. Corvid can go online only when the
install, deploy, operate, upgrade, support, and claim-audit paths are runnable
and honest.

## Launch Gates

- `corvid deploy package` emits deployable artifacts and evidence.
- At least one reference app has Compose, PaaS, Kubernetes, and systemd manifests.
- Release channels produce signed binaries, checksums, changelog, and notes.
- Upgrade tooling reports weakening guarantees before applying migrations.
- Maintainer and developer docs are complete enough for a clean-clone run.
- Final claims have runnable evidence or are removed.

## Release Channels

- `nightly`: daily development snapshot, no stability promise.
- `beta`: release-candidate channel for external app builders.
- `stable`: SemVer-governed channel, compatibility policy applies.

## Support Posture

- Security reports use the advisory process in maintainer docs.
- Production incidents list public contact paths and expected response windows.
- Beta feedback closes as code, docs, tests, or explicit non-scope.

## Security Process

- Release artifacts are signed.
- Checksums are published.
- Key rotation is documented before stable.
- Vulnerability disclosures are triaged before marketing launch.

## Beta Criteria

The beta program is real only when external developers build real backend apps.
Internal demos do not count toward the 20-developer target.

## Non-Scope

- Hosted Corvid Cloud.
- Guaranteed regulated-domain compliance.
- Autonomous financial, medical, legal, or destructive actions.
- Unreviewed external writes from reference apps.
