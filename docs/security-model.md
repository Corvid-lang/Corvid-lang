# Corvid Security Model

This document defines what Corvid's production launch claims rely on, what
the toolchain verifies, and what remains outside the trust boundary. It is
written for maintainers, host integrators, and reviewers evaluating signed
Corvid cdylibs.

The canonical machine-readable guarantee list is
`corvid_guarantees::GUARANTEE_REGISTRY`, rendered in
[`docs/core-semantics.md`](core-semantics.md). This document explains the
system boundary around that list; it does not add new guarantees.

## Launch Claim

A signed Corvid cdylib can claim only the guarantee ids carried in its
embedded ABI descriptor's `claim_guarantees` array. The signing path refuses
to emit a signed cdylib when the source declares a contract that is missing
from that signed claim set, when the claim set names an unknown guarantee id,
or when it names an `out_of_scope` guarantee.

The reviewer-facing commands are:

```text
corvid build app.cor --target=cdylib --sign=key.hex
corvid claim --explain target/release/libapp.so --key pub.hex --source app.cor
corvid-abi-verify --source app.cor target/release/libapp.so
corvid receipt verify-abi target/release/libapp.so --key pub.hex
```

`corvid claim --explain` is the safest public quote surface. It reports the
descriptor-carried guarantee ids, ABI surface summary, signing-key
fingerprint when a key is supplied, and independent source/binary descriptor
agreement when a source path is supplied.

## Trust Boundary

```text
Corvid source
    |
    v
parser -> resolver -> typechecker -> IR lowering
    |              |              |
    |              |              +-- static contract diagnostics
    |              +---------------- imported module/effect boundary checks
    +------------------------------- syntax and declaration shape
    |
    v
ABI emitter -> canonical CORVID_ABI_DESCRIPTOR
    |
    +--> claim coverage gate for --sign
    |       refuses unknown, out-of-scope, or missing guarantee ids
    |
    v
DSSE signer -> CORVID_ABI_ATTESTATION
    |
    v
codegen/linker -> cdylib exports descriptor + optional attestation
    |
    +--> corvid-abi-verify independently rebuilds descriptor from source
    +--> corvid receipt verify-abi verifies signature + descriptor match
    +--> host runtime loads and executes accepted binary
```

The trusted computing base for a signed cdylib is:

- Corvid parser, resolver, typechecker, IR lowering, ABI emitter, codegen, and
  runtime libraries.
- `corvid-guarantees`, because it defines the stable ids and enforcement
  classes.
- The claim coverage gate in `corvid build --sign`.
- The signing implementation and the private signing key at build time.
- The verifier tools used by the host: `corvid claim --explain`,
  `corvid-abi-verify`, and `corvid receipt verify-abi`.
- The Rust compiler, Cranelift, system linker, dynamic loader, operating
  system, and CPU executing the artifacts.

The slice 35-H bilateral verifier reduces but does not remove the compiler
TCB. It gives reviewers an independent descriptor reconstruction path and
catches source/binary descriptor disagreement; it does not prove that every
codegen instruction is semantically correct.

## Attacker Model

Corvid defends against these launch-relevant failures:

- Source authors accidentally or deliberately under-reporting declared
  effects, approval requirements, budget ceilings, confidence thresholds, or
  grounded provenance requirements.
- Module/import aliasing that attempts to hide a dangerous tool or weaken an
  imported effect surface.
- A cdylib descriptor being edited after signing: `verify-abi` checks that the
  signed payload matches the loaded `CORVID_ABI_DESCRIPTOR`.
- A source file and binary descriptor drifting apart: `corvid-abi-verify`
  rebuilds the descriptor from source and byte-compares it with the cdylib.
- A signed build claiming more than the registry enforces: `build --sign`
  refuses unknown, out-of-scope, or incomplete signed claim sets.
- A host receiving an unsigned cdylib while expecting attestation:
  `verify-abi` reports unsigned with a distinct exit code so host policy can
  reject it.

Corvid does not claim to defend against arbitrary malicious native code linked
beside Corvid, malicious host tools, or compromised infrastructure outside the
verification boundary below.

## Maintainer Rules

Every public safety or contract claim must have a stable guarantee id in
`GUARANTEE_REGISTRY`.

An enforced guarantee must be `static` or `runtime_checked` and must carry
positive and adversarial test references that compile and run. A documented
non-defense must be `out_of_scope` with an explicit reason.

When adding new source syntax that behaves like a contract, maintainers must
choose one of these before signing supports it:

- Map it to an existing non-out-of-scope guarantee id in the signed claim
  coverage gate.
- Add a new guarantee id, implementation, tests, and regenerated
  `docs/core-semantics.md`.
- Fail signed builds that use the syntax until the guarantee exists.

This is why `build --sign` currently fails closed for contract-like features
whose guarantee is not yet registered, such as advanced prompt dispatch policy.

## Host Acceptance Workflow

A production host that requires signed Corvid binaries should enforce:

```text
corvid receipt verify-abi <cdylib> --key <trusted-pubkey>
corvid-abi-verify --source <source.cor> <cdylib>
corvid claim --explain <cdylib> --key <trusted-pubkey> --source <source.cor>
```

Accept only when:

- `verify-abi` exits 0.
- `corvid-abi-verify` exits 0.
- `claim --explain` reports `attestation: verified`.
- `claim --explain` reports `source_descriptor_agreement: verified`.
- The printed `claim_guarantees` ids are acceptable for the host's policy.

Hosts that intentionally allow unsigned development builds must treat that as
a local policy exception, not as a Corvid safety guarantee.

## Non-Goals

Compromised host kernel:
Corvid assumes the operating system and privileged runtime environment do not
rewrite process memory or lie about loaded binaries. A compromised kernel can
defeat any user-space verifier.

Signing-key compromise:
Corvid signs and verifies. It does not rotate, revoke, escrow, or protect the
private signing key. Key management belongs to the host's security program.

Compiler-toolchain compromise:
Corvid trusts the Rust compiler, Cranelift, system linker, and platform loader.
The bilateral verifier catches descriptor disagreement, but it is not a
reproducible-build proof and does not defend against a malicious toolchain.

Provider honesty:
Corvid can type and record AI calls, approvals, costs, provenance, and replay
boundaries. It does not prove that an external model provider, search index,
or SaaS API returns truthful content.

Runtime budget termination:
Corvid enforces the current budget guarantee as a compile-time ceiling for
statically known costs. Live mid-execution termination on actual runtime cost
crossing remains explicitly out of scope until a runtime enforcement slice
ships and promotes `budget.runtime_termination`.

Application policy completeness:
Corvid can enforce contracts expressed in the language and registry. It does
not decide whether an application has expressed every legal, compliance,
privacy, or product policy its operator needs.
