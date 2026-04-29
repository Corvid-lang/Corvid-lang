# Corvid Beta Program

The beta program is not complete until at least 20 external developers build real backend applications and every feedback item is closed as code, docs, tests, or explicit non-scope. This file defines the intake and closure process; it is not evidence that the beta has happened.

## Intake

Each beta participant needs a tracking issue with:

- participant identifier,
- app category,
- repository or private evidence link,
- Corvid version and commit,
- operating system,
- commands run,
- deployment target attempted,
- blockers,
- feedback labels.

Required labels:

- `beta:intake`
- `beta:blocked`
- `beta:docs`
- `beta:bug`
- `beta:feature`
- `beta:non-scope`
- `beta:closed`

## Required Developer Run

The participant must run at least:

```bash
corvid check <app>/src/main.cor
corvid migrate status --dir <app>/migrations
corvid deploy package <app> --out target/<app>-package
corvid upgrade check <app>
```

For agent apps, the participant must also inspect approval boundaries, traces, and connector mode.

## Closure Rules

Feedback closes only when one of these is true:

- a code change lands with a regression test,
- a docs change lands with a docs coverage test,
- a new test captures the expected behavior,
- the issue is explicitly marked `beta:non-scope` with rationale.

## Completion Evidence

Stable launch requires:

- 20 closed participant issues,
- a closure summary with counts by label,
- links to code/docs/tests/non-scope decisions,
- unresolved risk list,
- maintainer signoff.

Current status: pending real external participants. Do not mark 43H1, 43H2, or 43H complete until issue evidence exists.
