# Claim Inventory

The launch claim inventory lives in `docs/launch-claim-audit.md` and is audited by:

```bash
corvid claim audit --json
```

Inventory rules:

- Every launch-facing claim must have a runnable command, linked committed artifact, or explicit blocked/non-scope status.
- Claims backed by `docs/core-semantics.md` must match the guarantee registry.
- Claims that depend on external beta feedback remain blocked until real issue evidence exists.
- Claims that use aspirational evidence wording fail the audit unless they are explicitly blocked or non-scope.

The inventory includes README-facing claims, docs-facing claims, release/deploy claims, and external beta status. Website and launch-page claims must be copied into the same table before public launch so `corvid claim audit` checks one source of truth.
