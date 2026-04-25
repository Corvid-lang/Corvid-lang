# Signed Dimension Artifacts

Custom dimensions change the compiler's safety algebra. A shared dimension must
therefore ship more than a TOML declaration: it needs a signed declaration,
optional machine-checkable proof, and regression programs that keep the claimed
semantics honest.

`corvid add-dimension ./freshness.dim.toml` accepts two local forms:

- A plain development fragment with one `[effect-system.dimensions.<name>]` section.
- A signed artifact with `[artifact]`, one dimension declaration, and optional `[[regression]]` entries.

Registry-hosted dimensions will use the same artifact format. The host is not
special; it only distributes artifacts that the local toolchain can verify.

## Artifact Shape

```toml
[artifact]
name = "freshness"
version = "1.0.0"
signing_key = "<ed25519 public key as hex>"
signature = "<ed25519 signature as hex>"

[effect-system.dimensions.freshness]
composition = "Max"
type = "timestamp"
default = "0"
semantics = "maximum data age in the call chain"
proof = "proofs/freshness_max.lean"

[[regression]]
name = "freshness_compiles"
expect = "compile"
source = '''
effect stale:
    freshness: 2

tool read_cache() -> String uses stale

agent main() -> String:
    return read_cache()
'''
```

## Verification

Installation fails closed unless all of these pass:

1. The artifact contains exactly one dimension declaration.
2. The declaration name matches `[artifact].name`.
3. The version is valid semver.
4. The Ed25519 signature verifies against Corvid's canonical artifact payload.
5. The normal dimension validation accepts the declaration.
6. The archetype law checker accepts the dimension.
7. Any declared Lean/Coq proof replays successfully.
8. Every regression program matches its declared `expect = "compile"` or `expect = "error"`.

If a signed artifact passes, only the dimension declaration is appended to the
project's `corvid.toml`. The artifact metadata remains a distribution contract,
not runtime project configuration.

## Why This Matters

Most package registries distribute code. Corvid's dimension registry distributes
pieces of the type system. The signature proves who authored the dimension; the
proof and law checks prove the algebraic rule is coherent; the regression corpus
proves known examples continue to behave as claimed.
