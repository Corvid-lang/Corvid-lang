#!/usr/bin/env sh
set -eu

if ! command -v cargo >/dev/null 2>&1; then
  echo "error: cargo is required. Install Rust from https://rustup.rs first." >&2
  exit 1
fi

echo "Installing corvid-cli with cargo..."
cargo install --path crates/corvid-cli --locked

if command -v rustup >/dev/null 2>&1; then
  rustup target add wasm32-unknown-unknown >/dev/null 2>&1 || true
fi

echo
echo "Installed. Run:"
echo "  corvid doctor"
echo "  corvid tour --list"
