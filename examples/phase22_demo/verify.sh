#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
source "$ROOT/examples/_bundle_demo_common.sh"

BUNDLE="$ROOT/examples/phase22_demo"
BASE="$ROOT/examples/phase22_demo_base"

expect_success_stdout_contains "bundle OK: phase22-demo (x86_64-unknown-linux-gnu)" \
  bundle verify "$BUNDLE"

if [[ "$(uname -s)" == "Linux" ]]; then
  expect_success_stdout_contains "bundle OK: phase22-demo (x86_64-unknown-linux-gnu)" \
    bundle verify "$BUNDLE" --rebuild
fi

expect_success_stdout_contains "\"descriptor_hash_changed\": true" \
  bundle diff "$BASE" "$BUNDLE" --json

expect_success_stdout_contains "approval-gated agents: issue_tag" \
  bundle audit "$BUNDLE" --question "Which agents require approval?" --json

expect_success_stdout_contains "\"trace_count\": 1" \
  bundle explain "$BUNDLE" --json

expect_success_stdout_contains "CC7.2" \
  bundle report "$BUNDLE" --format soc2 --json

expect_success_stdout_contains "agent.replayable_gained:classify" \
  bundle query "$BUNDLE" --delta agent.replayable_gained:classify --json

expect_success_stdout_contains "\"signature_verified\": true" \
  bundle lineage "$BUNDLE" --json
