# Package Imports

Corvid package imports are designed as content-addressed trust boundaries.
The source program names a semantic package URI:

```corvid
import "corvid://@anthropic/safety-baseline/v2.3" as safety
```

The compiler never resolves that URI by floating network state. It must be
locked in `Corvid.lock`:

```toml
[[package]]
uri = "corvid://@anthropic/safety-baseline/v2.3"
url = "https://registry.corvid.dev/@anthropic/safety-baseline/v2.3/policy.cor"
sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
registry = "https://registry.corvid.dev"
signature = "ed25519:..."
```

The current compiler behavior is intentionally fail-closed:

- Missing `Corvid.lock` rejects every `corvid://...` package import.
- Missing lock entries reject the specific package URI.
- The locked URL is fetched as remote Corvid source.
- The locked SHA-256 is verified over exact response bytes before lexing,
  parsing, resolving, typechecking, or lowering.
- Inline `hash:sha256:...` is rejected on package imports; package hashes live
  in the lockfile so source code stays semantic and the lockfile stays the
  reproducibility authority.

This is the package-resolution foundation. Registry index resolution,
semantic-version selection, and signed publish verification are separate
follow-up slices; they should extend the lockfile contract rather than bypass
it.

## Adding Packages

`corvid add` resolves a semantic package request through a registry index and
writes both the project manifest dependency and the concrete lock entry:

```text
corvid add @anthropic/safety-baseline@2.3 --registry ./registry/index.toml
```

The registry index is TOML:

```toml
[[package]]
name = "@anthropic/safety-baseline"
version = "2.3.4"
uri = "corvid://@anthropic/safety-baseline/v2.3.4"
url = "https://registry.corvid.dev/@anthropic/safety-baseline/v2.3.4/policy.cor"
sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
signature = "ed25519:..."
```

Resolution is semantic-version aware. A request for `@scope/name@2.3` selects
the highest `2.3.x` version in the index. Explicit semver requirements such as
`@scope/name@^2.3.0` are also accepted.

Before writing `Corvid.lock`, `corvid add` fetches the selected source, verifies
the registry hash, parses the module, computes its exported semantic summary,
and applies project policy from `corvid.toml`:

```toml
[package-policy]
allow-approval-required = false
allow-effect-violations = false
require-deterministic = true
require-replayable = true
require-package-signatures = true
```

Rejected packages do not write or modify the lockfile. Accepted packages store
their semantic summary in `Corvid.lock` so code review can see both the bytes
and the AI-safety contract that those bytes export.

The project manifest is updated under `[dependencies]`. When a registry is
explicitly selected, the entry is table-shaped so the source of future updates
is reproducible:

```toml
[dependencies."@anthropic/safety-baseline"]
version = "2.3"
registry = "./registry/index.toml"
```

## Removing and Updating Packages

`corvid remove @scope/name` removes the dependency from `corvid.toml` and every
matching `corvid://@scope/name/...` entry from `Corvid.lock`.

`corvid update @scope/name` reads the existing `[dependencies]` version
requirement and registry, resolves the newest matching package, re-verifies the
source hash/signature/semantic policy, and rewrites the lock entry. Passing a
full spec such as `corvid update @scope/name@^2.0.0 --registry ./index.toml`
also updates the manifest requirement.

## Lockfile Conflict Verification

`corvid package verify-lock` checks that the local package graph is still
coherent:

```text
corvid package verify-lock
corvid package verify-lock --json
```

The verifier checks:

- every manifest dependency has a matching locked package;
- locked versions satisfy the manifest version requirement;
- one dependency is not locked to multiple versions at once;
- duplicate package URIs and duplicate locked versions are reported;
- stale lock entries without a matching manifest dependency are reported;
- every locked package has a semantic summary;
- the locked semantic summary still satisfies the current `[package-policy]`.

That last check is important. If project policy changes from permissive to
strict, the existing lockfile is re-evaluated without reinstalling packages.
The package manager therefore treats effect profiles, approvals, replayability,
determinism, grounded outputs, and signatures as compatibility constraints, not
as comments.

## Publishing Signed Packages

`corvid package publish` creates the source artifact, computes its hash and
semantic summary, signs the package subject with Ed25519, and updates the
registry index:

```text
corvid package publish policy.cor \
  --name @anthropic/safety-baseline \
  --version 2.3.4 \
  --out ./registry \
  --url-base https://registry.corvid.dev/@anthropic/safety-baseline \
  --key 0000000000000000000000000000000000000000000000000000000000000000 \
  --key-id anthropic-release
```

The signature covers:

- package name and semantic version
- `corvid://...` package URI
- artifact URL
- SHA-256 digest
- exported semantic summary

`corvid add` verifies any `ed25519:<key-id>:<public-key>:<signature>` entry it
finds in the index. If `require-package-signatures = true`, unsigned entries are
rejected before the lockfile changes.

## Registry Contract Verification

`corvid package verify-registry <index>` checks that a registry is a stateless
content-addressed source registry, not a trusted server:

```text
corvid package verify-registry ./registry/index.toml
corvid package verify-registry https://registry.corvid.dev/index.toml --json
```

The verifier checks each index entry for:

- scoped package name and valid semantic version;
- canonical `corvid://@scope/name/vX.Y.Z` URI when present;
- immutable HTTP(S) `.cor` artifact URL with the concrete version in the path;
- no query strings or fragments in artifact URLs;
- 64-character SHA-256 digest;
- artifact `Cache-Control` containing `max-age=` and `immutable`;
- fetched artifact bytes matching the advertised SHA-256;
- valid UTF-8 Corvid source;
- semantic summary in the index matching the source, when present;
- Ed25519 signature validity, when present;
- no duplicate `(name, version)` entries.

A registry can therefore be hosted by static files and a CDN. The compiler and
CLI verify the bytes, signature, and semantic contract themselves.

## Package Metadata Pages

`corvid package metadata` renders a package page from the compiler's semantic
summary:

```text
corvid package metadata policy.cor \
  --name @anthropic/safety-baseline \
  --version 2.3.4 \
  --signature ed25519:anthropic-release:...
```

The Markdown output is suitable for a registry package page. The JSON output is
the same data for web frontends:

```text
corvid package metadata policy.cor --name @scope/name --version 1.0.0 --json
```

Metadata pages show:

- package identity, canonical `corvid://...` URI, and install command;
- supplied signature provenance, or `not supplied` for local source-only pages;
- public exports and their declaration kind;
- exported effect names and dangerous approval boundaries;
- grounded source/return guarantees;
- replayability and determinism declarations;
- cost notes and effect-violation counts for exported agents.

The page is intentionally generated from compiler facts rather than handwritten
README claims. If a package changes its effects, approval boundary, grounding,
or replay contract, the metadata page changes with the source.
