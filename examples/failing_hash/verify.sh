#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
source "$ROOT/examples/_bundle_demo_common.sh"

expect_fail_contains "BundleHashMismatch" \
  bundle verify "$ROOT/examples/failing_hash"
