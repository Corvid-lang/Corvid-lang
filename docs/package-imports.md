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
