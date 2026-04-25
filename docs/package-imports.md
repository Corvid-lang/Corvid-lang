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
writes the concrete lock entry:

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
```

Rejected packages do not write or modify the lockfile. Accepted packages store
their semantic summary in `Corvid.lock` so code review can see both the bytes
and the AI-safety contract that those bytes export.
