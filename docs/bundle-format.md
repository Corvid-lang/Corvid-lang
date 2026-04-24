# Corvid Bundle Format

Corvid bundles are reproducibility-spec directories for exported `pub extern "c"` agents.

A bundle makes one cdylib release auditable without network access:

- the compiled library
- the emitted ABI descriptor
- projected Rust and Python bindings
- recorded execution traces
- signed receipt material for offline verification
- lineage links to predecessor bundles

The reference example lives in [examples/phase22_demo](../examples/phase22_demo/verify.sh). Its predecessor bundle lives in [examples/phase22_demo_base](../examples/phase22_demo_base/).

## Goals

Bundles are designed to answer seven questions deterministically:

1. Did the committed bundle contents change?
2. Can the library, descriptor, and bindings be rebuilt from source?
3. What changed between two bundle revisions?
4. What does the bundle expose to a host?
5. How does the bundle map to audit controls?
6. What single attestation delta explains the behavior drift?
7. What signed predecessor chain led to this bundle?

Those questions map directly to the bundle CLI:

- `corvid bundle verify`
- `corvid bundle verify --rebuild`
- `corvid bundle diff`
- `corvid bundle audit`
- `corvid bundle explain`
- `corvid bundle report --format soc2`
- `corvid bundle query`
- `corvid bundle lineage`

## Directory Shape

A bundle is a directory containing a root manifest named `corvid-bundle.toml`.

Typical layout:

```text
phase22_demo/
├── artifacts/
│   ├── libcorvid_test_tools.a
│   └── release/
│       ├── classify.corvid-abi.json
│       ├── libclassify.so
│       └── lib_classify.h
├── bindings_python/
├── bindings_rust/
├── keys/
│   ├── receipt.envelope.json
│   └── verify.hex
├── src/
│   └── classify.cor
├── traces/
│   └── safe.jsonl
├── corvid-bundle.toml
└── verify.sh
```

Notes:

- The committed demo artifacts are Linux-only: `x86_64-unknown-linux-gnu`.
- The committed `safe.jsonl` traces in the phase-22 demo were recorded on the Windows development box and are replayed against the Linux artifact for rebuild verification.
- The bundle does not depend on `target/` contents being committed. Public artifacts live under `artifacts/`.

## Manifest Schema

The current schema version is `1`.

Example:

```toml
bundle_schema_version = 1
name = "phase22-demo"
target_triple = "x86_64-unknown-linux-gnu"
primary_source = "src/classify.cor"
tools_staticlib_path = "artifacts/libcorvid_test_tools.a"
library_path = "artifacts/release/libclassify.so"
descriptor_path = "artifacts/release/classify.corvid-abi.json"
header_path = "artifacts/release/lib_classify.h"
bindings_rust_dir = "bindings_rust"
bindings_python_dir = "bindings_python"
receipt_envelope_path = "keys/receipt.envelope.json"
receipt_verify_key_path = "keys/verify.hex"

[lineage]
bundle_id = "phase22-demo"

[[lineage.predecessors]]
name = "base"
path = "../phase22_demo_base"
relation = "parent"

[[traces]]
name = "safe"
path = "traces/safe.jsonl"
source = "src/classify.cor"
sha256 = "<trace sha256>"
expected_agent = "classify"
expected_result_json = "\"positive\""
expected_grounded_sources = []
expected_observation = true

[hashes]
library = "<sha256>"
descriptor = "<sha256>"
header = "<sha256>"
bindings_rust = "<directory hash>"
bindings_python = "<directory hash>"
receipt_envelope = "<sha256>"
receipt_verify_key = "<sha256>"
tools_staticlib = "<sha256>"
```

### Required Fields

- `bundle_schema_version`
- `name`
- `target_triple`
- `primary_source`
- `library_path`
- `descriptor_path`
- `bindings_rust_dir`
- `bindings_python_dir`
- `traces`
- `hashes`

### Optional Fields

- `tools_staticlib_path`
- `header_path`
- `capsule_path`
- `receipt_envelope_path`
- `receipt_verify_key_path`
- `lineage`

## Hash Semantics

File hashes are SHA-256 over file bytes.

Directory hashes are SHA-256 over a stable concatenation of:

1. relative path bytes with `/` separators
2. a zero byte separator
3. little-endian file length bytes
4. file bytes

Files are collected recursively and sorted lexicographically by relative path before hashing.

The verifier checks committed hashes first. A hash mismatch fails immediately with `BundleHashMismatch`.

## Signed Receipts

If both `receipt_envelope_path` and `receipt_verify_key_path` are present, Corvid verifies the DSSE envelope offline during:

- `corvid bundle verify`
- `corvid bundle lineage`

The envelope payload type is `application/vnd.corvid-receipt+json`.

If the receipt hash matches but the signature does not, verification fails with `BundleSignatureVerifyFailed`.

## Lineage

Bundles can declare predecessor edges:

```toml
[lineage]
bundle_id = "phase22-demo"

[[lineage.predecessors]]
name = "base"
path = "../phase22_demo_base"
relation = "parent"
```

`corvid bundle lineage` walks the predecessor DAG, verifies every reachable signed receipt, and rejects:

- missing predecessor declarations with `BundleLineageMissing`
- ambiguous unnamed predecessor selection with `BundlePredecessorAmbiguous`
- cycles with `BundleLineageCycle`
- missing receipt/key pairs with `BundleLineageSignatureMissing`
- bad signatures with `BundleSignatureVerifyFailed`

## Counterfactual Query

`corvid bundle query` compares a bundle to one predecessor and asks:

> if only delta X had landed, what attestation diff would remain?

Current demo usage:

```bash
corvid bundle query examples/phase22_demo \
  --delta agent.replayable_gained:classify \
  --json
```

The current query engine consumes the trace-diff delta-isolation machinery read-only and only supports the delta classes the isolation layer can currently synthesize. Unsupported classes fail with `BundleCounterfactualUnsupported`.

## Rebuild Verification

`corvid bundle verify --rebuild` proves three things:

1. descriptor rebuild matches the committed descriptor bytes
2. cdylib rebuild matches the committed library bytes
3. Rust and Python binding projection matches the committed binding trees

Then each recorded trace is replayed against the rebuilt library and checked against:

- `expected_agent`
- `expected_result_json`
- `expected_observation`

The committed demo bundle is Linux-only, so `--rebuild` is expected to run on Linux. On a host with a different target triple, rebuild verification fails with `BundlePlatformUnsupported`.

## Public Demo and Failing Cases

The shipped examples are:

- [examples/phase22_demo](../examples/phase22_demo/verify.sh): happy-path public spec bundle
- [examples/phase22_demo_base](../examples/phase22_demo_base/): predecessor bundle for diff/query/lineage
- [examples/failing_hash](../examples/failing_hash/verify.sh): committed hash mismatch
- [examples/failing_signature](../examples/failing_signature/verify.sh): receipt signature failure
- [examples/failing_rebuild](../examples/failing_rebuild/verify.sh): rebuild mismatch
- [examples/failing_lineage](../examples/failing_lineage/verify.sh): predecessor signature failure
- [examples/failing_adversarial](../examples/failing_adversarial/verify.sh): four guard cases, including deterministic offline audit behavior under a hostile question

The GitHub Actions launch gate runs the happy bundle script, every failing-case script, and the focused `bundle_*` test binaries on Linux.
