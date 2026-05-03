# Corvid Package Manager — Scope and Honest Boundary

This document defines what the Corvid package manager does today, what it does
not do, and where the boundary between "format and tooling" and "hosted
service" lives. Phase 25 of the ROADMAP shipped the format-and-tooling layer.
A hosted registry service does not run as part of v1.0.

## What ships today

- **Package format.** A Corvid package is a single `.cor` source artifact with
  a stable URI of the shape `corvid://<scope>/<name>@<semver>`, an SHA-256
  content hash, and an Ed25519 signature over `(uri, sha256, semantic_summary)`
  produced by the publisher's signing key.
- **Lockfile.** `Corvid.lock` records exact resolved versions, content hashes,
  semantic summaries, and signed publish metadata for every dependency in the
  graph. The driver fails closed if a `corvid://` import is missing from the
  lock or if the locked URL bytes do not hash to the lockfile's recorded value.
- **Effect-aware resolution.** `corvid add` records the imported package's
  semantic summary into the lock. Project-level policy can fail or warn when
  a transitive dependency exceeds declared trust, cost, data-class, replay,
  approval, or grounded-output limits.
- **CLI surface.**

  ```
  corvid package publish <source.cor> --name=<scope>/<name> --version=<semver> \
    --out=<registry-dir> --url-base=<https://...|file://...> --key=<seed-hex>
  corvid package metadata <source.cor> --name=<...> --version=<...>
  corvid package verify-registry --registry=<dir>
  corvid package verify-lock
  corvid add <package>@<semver> --registry=<index.toml|dir|url>
  corvid remove <package>
  corvid update [<package>]
  ```

- **Registry index format.** `publish` writes a JSON registry index entry that
  pins the canonical URI, source artifact URL, SHA-256, semantic summary, and
  signature. The index is the format any host can serve from a static webroot
  or CDN; immutability of versioned artifacts is recommended via
  `Cache-Control: public, max-age=31536000, immutable`.
- **Verification commands.** `corvid package verify-registry` validates index
  entries against the format. `corvid package verify-lock` validates the
  installed graph against the manifest, lock, and current policy.

## What does NOT ship today

- **A hosted registry service at `registry.corvid.dev` or anywhere else.**
  No production registry runs. No package has been published to a public
  index Corvid maintainers operate. The CLI's `--url-base` accepts `file://`
  and any http endpoint a user runs themselves; Corvid does not provide one.
- **Discovery / search.** There is no `corvid search` against a public index.
- **Package pages on a website.** `corvid package metadata` renders the same
  data a website would; no website serves it.
- **Transitive dependency installation from a public index.** `corvid add`
  works against any registry index URL, directory, or `index.toml` path the
  user supplies; the user is responsible for pointing it at a location that
  hosts the registry index format above.
- **Yanking, deprecation, ownership transfer, account systems.** No registry
  service means no operational surface for these.

## Why this is honest, not aspirational

The format and CLI are the load-bearing parts of a package manager. A hosted
registry service is operations work — a stateless HTTP API plus a CDN plus
account management plus an abuse policy plus support — and operating one
before there is real demand is the kind of premature infrastructure the
no-shortcuts rule rejects. Users who need a registry right now run their own
against the published format; that is the only honest claim the project can
make about hosted distribution at v1.0.

When a hosted service does land, it is a separate phase (post-v1.0 per the
ROADMAP's Post-v1.0 Roadmap section) and the ROADMAP entry will name it
explicitly. Until then, this document is the source of truth for the
boundary.

## How to read the ROADMAP slice marks

`Phase 25` slice checklist items `[x]` track *format-and-tooling* completion.
The phase's `Non-scope` section already says:

> Private registries (post-v1.0). Binary package distribution (post-v1.0 — all
> v1.0 packages are source).

This document expands the same boundary to cover the absence of a public
hosted registry service: it is not a private-registry feature gap, it is the
fact that no Corvid-operated public service runs.

## Where the boundary appears in the codebase

- `crates/corvid-driver/src/package_registry.rs` — `publish_package()` writes
  the index entry and `corvid add` requires an explicit registry location
  through `--registry`, the manifest, or `CORVID_PACKAGE_REGISTRY`.
- `crates/corvid-driver/src/package_manifest.rs` — manifest parser.
- `crates/corvid-driver/src/package_lock.rs` — lockfile reader/writer.
- `crates/corvid-driver/src/package_version.rs` — semver resolution.
- `crates/corvid-driver/src/package_conflicts.rs` — conflict detection.
- `crates/corvid-driver/src/package_metadata.rs` — metadata renderer.
- `crates/corvid-driver/src/package_policy.rs` — policy enforcement.
- `crates/corvid-cli/src/main.rs` — `Package` subcommand and dispatch.

`grep` against any of these files returns zero `https://registry.corvid.dev`
references. The format is what Corvid ships.

## Registry guarantee row

The registry entry `package.hosted_registry_available` is registered as
`OutOfScope` with the reason: *"no hosted Corvid package registry service runs
yet; `--url-base` accepts file:// and any http endpoint a user runs
themselves."*

Anyone reading `corvid contract list --class=out_of_scope` will see this
boundary alongside the other explicit non-defenses.
