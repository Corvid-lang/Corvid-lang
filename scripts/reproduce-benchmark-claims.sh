#!/usr/bin/env sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"

echo "Reproducing published benchmark claim surfaces"
echo
echo "[1/4] Marketable session vs Python"
cargo run -q -p corvid-cli -- bench compare python --session 2026-04-17-marketable-session
echo
echo "[2/4] Marketable session vs JS"
cargo run -q -p corvid-cli -- bench compare js --session 2026-04-17-marketable-session
echo
echo "[3/4] Corrected session vs Python"
cargo run -q -p corvid-cli -- bench compare python --session 2026-04-17-corrected-session
echo
echo "[4/4] Corrected session vs JS"
cargo run -q -p corvid-cli -- bench compare js --session 2026-04-17-corrected-session
