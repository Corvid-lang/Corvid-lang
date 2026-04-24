#!/usr/bin/env bash
set -euo pipefail

run_corvid() {
  cargo run -q -p corvid-cli -- "$@"
}

expect_fail_contains() {
  local needle="$1"
  shift

  local stdout_file stderr_file
  stdout_file="$(mktemp)"
  stderr_file="$(mktemp)"
  if run_corvid "$@" >"$stdout_file" 2>"$stderr_file"; then
    echo "expected failure containing: $needle" >&2
    cat "$stdout_file" >&2 || true
    cat "$stderr_file" >&2 || true
    rm -f "$stdout_file" "$stderr_file"
    exit 1
  fi
  if ! grep -Fq "$needle" "$stderr_file"; then
    echo "stderr did not contain expected marker: $needle" >&2
    cat "$stderr_file" >&2 || true
    rm -f "$stdout_file" "$stderr_file"
    exit 1
  fi
  rm -f "$stdout_file" "$stderr_file"
}

expect_success_stdout_contains() {
  local needle="$1"
  shift

  local stdout_file stderr_file
  stdout_file="$(mktemp)"
  stderr_file="$(mktemp)"
  run_corvid "$@" >"$stdout_file" 2>"$stderr_file"
  if ! grep -Fq "$needle" "$stdout_file"; then
    echo "stdout did not contain expected marker: $needle" >&2
    cat "$stdout_file" >&2 || true
    cat "$stderr_file" >&2 || true
    rm -f "$stdout_file" "$stderr_file"
    exit 1
  fi
  rm -f "$stdout_file" "$stderr_file"
}
