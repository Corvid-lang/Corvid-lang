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

## Registry Index

`corvid add-dimension freshness@1.0.0` resolves through an effect-registry index.
By default the client reads `https://effect.corvid-lang.org/index.toml`. During
development or private distribution, set `CORVID_EFFECT_REGISTRY` or pass
`corvid add-dimension freshness@1.0.0 --registry ./registry`.

The registry index is intentionally small:

```toml
[[dimension]]
name = "freshness"
version = "1.0.0"
url = "artifacts/freshness-1.0.0.dim.toml"
sha256 = "<artifact sha256 hex>"
proof_url = "proofs/freshness_max.lean"
proof_sha256 = "<proof sha256 hex>"
```

`proof_url` and `proof_sha256` are optional, but if either is present both must
be present. Local registry directories, local `index.toml` files, and HTTP(S)
indexes use the same shape. Relative artifact and proof paths resolve from the
index location.

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

Registry resolution adds one earlier gate: the fetched artifact bytes must match
the index's SHA-256 digest before signature verification runs. A compromised CDN
therefore cannot silently swap the artifact even before the Ed25519 signature
check catches the tampering.

## Why This Matters

Most package registries distribute code. Corvid's dimension registry distributes
pieces of the type system. The signature proves who authored the dimension; the
proof and law checks prove the algebraic rule is coherent; the regression corpus
proves known examples continue to behave as claimed.
