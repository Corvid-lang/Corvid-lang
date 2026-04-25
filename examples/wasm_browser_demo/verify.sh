#!/usr/bin/env sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)"
DEMO="$ROOT/examples/wasm_browser_demo"
SOURCE="$DEMO/src/refund_gate.cor"
OUT="$DEMO/target/wasm"

cargo run -q -p corvid-cli -- build "$SOURCE" --target=wasm

for name in refund_gate.wasm refund_gate.js refund_gate.d.ts refund_gate.corvid-wasm.json; do
  test -f "$OUT/$name"
done

grep -q "kind: 'approval_decision'" "$OUT/refund_gate.js"
grep -q "kind: 'run_completed'" "$OUT/refund_gate.js"
grep -q "CorvidWasmHost" "$OUT/refund_gate.d.ts"
grep -q "review_refund(amount: bigint): bigint" "$OUT/refund_gate.d.ts"
grep -q "../target/wasm/refund_gate.js" "$DEMO/web/demo.js"

echo "wasm browser demo OK"
