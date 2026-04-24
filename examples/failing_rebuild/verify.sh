#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
source "$ROOT/examples/_bundle_demo_common.sh"

if [[ "$(uname -s)" == "Linux" ]]; then
  expect_fail_contains "BundleRebuildMismatch" \
    bundle verify "$ROOT/examples/failing_rebuild" --rebuild
else
  expect_fail_contains "BundlePlatformUnsupported" \
    bundle verify "$ROOT/examples/failing_rebuild" --rebuild
fi
