#!/usr/bin/env sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"

echo "Reproducing bundle verification claims"
echo
if [ -x examples/phase22_demo/verify.sh ]; then
  echo "[1/2] Happy-path bundle verification"
  sh examples/phase22_demo/verify.sh
else
  echo "Missing examples/phase22_demo/verify.sh" >&2
  exit 1
fi

echo
echo "[2/2] Negative-path bundle verification"
for dir in failing_hash failing_signature failing_rebuild failing_lineage failing_adversarial; do
  sh "examples/$dir/verify.sh"
done
