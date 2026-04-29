# Launch Claim Audit

Every launch-facing claim should point at a runnable command, test, or committed artifact.

| Claim | Evidence |
|---|---|
| Approval boundaries are compiler-visible | `corvid tour --topic approve-gates` |
| Grounded values carry provenance | `corvid tour --topic grounded-values`; `corvid trace dag <trace>` |
| Replay is deterministic | `corvid replay <trace> --source <file>` |
| Project safety surface can be audited | `corvid audit <file>` |
| Environment prerequisites are machine-checkable | `corvid doctor` |
| WASM build is shipped | `corvid build <file> --target=wasm` |
| Package metadata exposes semantic contracts | `corvid package metadata <file> --name @scope/name --version 1.0.0` |
| Bundle verification is auditable | `corvid bundle verify <bundle>` and `corvid bundle audit <bundle>` |
| Signed cdylib claims are externally explainable | `corvid claim --explain <cdylib> --key <pubkey> --source <file.cor>` |
| Deploy packages include signed build attestation | `corvid deploy package <app> --out <dir>` |
| Release channels emit signed artifacts | `corvid release beta 1.0.0-beta.1 --out <dir>` |
| Upgrade migrations are machine-checkable | `corvid upgrade check <app> --json` |
| Claim inventory is machine-auditable | `corvid claim audit --json` |
| External beta feedback is complete | blocked: requires 20 real external developers and closed feedback issues |

Claims that depend on benchmark numbers, external beta feedback, or launch media stay blocked until the corresponding artifact is checked in or published.
