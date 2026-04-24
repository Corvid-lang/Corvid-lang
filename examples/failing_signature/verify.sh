#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
source "$ROOT/examples/_bundle_demo_common.sh"

expect_fail_contains "BundleSignatureVerifyFailed" \
  bundle verify "$ROOT/examples/failing_signature"
