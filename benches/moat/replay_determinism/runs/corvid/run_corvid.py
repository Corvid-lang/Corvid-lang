"""Corvid replay-determinism harness.

Invokes `cargo run -p refund_bot_demo` N times against the
mocked-tools / mocked-LLM demo runner, normalizes the resulting
JSONL traces, and asks whether all N runs are byte-identical
after normalization.

Normalization (matches the rules in ../../README.md):

  - Any `ts_ms` field      → `<TS>`
  - Any `run_id` field     → `<RUN_ID>`
  - On `seed_read` events with `purpose = "rollout_default_seed"`,
    the `value` field      → `<SEED>`

Anything else that varies across runs (tool args, tool results,
LLM prompt / rendered / result, approval labels, final result)
will surface as a real divergence.

Usage:
    python benches/moat/replay_determinism/runs/corvid/run_corvid.py \\
        --n 20 \\
        --out benches/moat/replay_determinism/runs/corvid/_summary.json
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path


def find_workspace_root(start: Path) -> Path:
    """Walk up from `start` until we find Cargo.toml + a `crates/` dir."""
    here = start.resolve()
    while True:
        if (here / "Cargo.toml").exists() and (here / "crates").is_dir():
            return here
        if here.parent == here:
            raise SystemExit(
                f"could not locate workspace root from {start} "
                "(looking for Cargo.toml + crates/)"
            )
        here = here.parent


def normalize_trace_bytes(raw: str) -> str:
    """Apply the documented normalization rules.

    Each rule replaces a per-execution non-deterministic field with a
    sentinel token. Anything NOT listed below — tool args, tool results,
    LLM prompts, LLM rendered text, LLM results, approval labels and
    args, final result — must be byte-identical across runs of the
    same agent on the same inputs.

    Wall-clock fields:
        - `ts_ms`, any field whose name ends in `_at_ms` (e.g.
          `issued_at_ms`, `expires_at_ms`) → `<TS>`

    Per-execution identifiers:
        - `run_id`     → `<RUN_ID>`
        - `token_id`   → `<TOKEN_ID>`   (fresh random per approval token)

    Seeded entropy reads:
        - On `seed_read` events with `purpose = "rollout_default_seed"`,
          the `value` field → `<SEED>`
    """
    out_lines: list[str] = []
    for line in raw.splitlines():
        if not line.strip():
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError as exc:
            raise SystemExit(
                f"trace line is not JSON: {line!r} ({exc})"
            ) from exc
        for key in list(event.keys()):
            if key == "ts_ms" or key.endswith("_at_ms"):
                event[key] = "<TS>"
            elif key == "run_id":
                event[key] = "<RUN_ID>"
            elif key == "token_id":
                event[key] = "<TOKEN_ID>"
        if (
            event.get("kind") == "seed_read"
            and event.get("purpose") == "rollout_default_seed"
            and "value" in event
        ):
            event["value"] = "<SEED>"
        out_lines.append(
            json.dumps(event, sort_keys=True, separators=(",", ":"))
        )
    return "\n".join(out_lines) + "\n"


def collect_existing_traces(trace_dir: Path) -> set[Path]:
    if not trace_dir.exists():
        return set()
    return {p for p in trace_dir.iterdir() if p.is_file()}


def run_once(workspace_root: Path, trace_dir: Path, attempt: int) -> Path:
    """Invoke `cargo run -p refund_bot_demo` once. Return the new trace path."""
    before = collect_existing_traces(trace_dir)
    cmd = ["cargo", "run", "-q", "-p", "refund_bot_demo"]
    proc = subprocess.run(
        cmd,
        cwd=workspace_root,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    if proc.returncode != 0:
        raise SystemExit(
            f"refund_bot_demo run #{attempt} failed (exit {proc.returncode})\n"
            f"stdout:\n{proc.stdout}\nstderr:\n{proc.stderr}"
        )
    after = collect_existing_traces(trace_dir)
    new_files = after - before
    if not new_files:
        # The runner uses `now_ms()` for the run_id — if two runs land
        # in the same millisecond the second one would clobber the
        # first. Sleep briefly and surface a clearer error if it
        # happens twice in a row.
        raise SystemExit(
            f"refund_bot_demo run #{attempt} did not produce a new "
            f"trace file under {trace_dir} — possible run_id collision"
        )
    if len(new_files) > 1:
        raise SystemExit(
            f"refund_bot_demo run #{attempt} produced multiple new trace "
            f"files: {sorted(new_files)}"
        )
    return next(iter(new_files))


def first_line_diff(a: str, b: str) -> tuple[int, str, str] | None:
    a_lines = a.splitlines()
    b_lines = b.splitlines()
    for i, (la, lb) in enumerate(zip(a_lines, b_lines)):
        if la != lb:
            return (i, la, lb)
    if len(a_lines) != len(b_lines):
        return (
            min(len(a_lines), len(b_lines)),
            "<eof>" if len(a_lines) < len(b_lines) else a_lines[-1],
            "<eof>" if len(b_lines) < len(a_lines) else b_lines[-1],
        )
    return None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--n", type=int, default=20)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument(
        "--workspace-root",
        type=Path,
        default=None,
        help="Override workspace root (default: walk up from this script).",
    )
    args = parser.parse_args()

    if args.workspace_root is not None:
        workspace_root = args.workspace_root.resolve()
    else:
        workspace_root = find_workspace_root(Path(__file__).parent)

    trace_dir = (
        workspace_root
        / "examples"
        / "refund_bot_demo"
        / "target"
        / "trace"
    )

    # Pre-build so we don't conflate compile time with run time.
    print(f"[corvid] cargo build -p refund_bot_demo (warmup)", file=sys.stderr)
    proc = subprocess.run(
        ["cargo", "build", "-q", "-p", "refund_bot_demo"],
        cwd=workspace_root,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    if proc.returncode != 0:
        raise SystemExit(
            f"cargo build failed (exit {proc.returncode})\n"
            f"stdout:\n{proc.stdout}\nstderr:\n{proc.stderr}"
        )

    print(f"[corvid] running N={args.n} times", file=sys.stderr)
    traces: list[str] = []
    trace_paths: list[Path] = []
    for i in range(args.n):
        # Brief delay so the millisecond-resolution run_id stays unique.
        if i > 0:
            time.sleep(0.005)
        new_trace = run_once(workspace_root, trace_dir, attempt=i + 1)
        raw = new_trace.read_text(encoding="utf-8")
        normalized = normalize_trace_bytes(raw)
        traces.append(normalized)
        trace_paths.append(new_trace)
        print(
            f"[corvid] run {i + 1}/{args.n} ok ({len(raw)} bytes raw, "
            f"{len(normalized)} normalized)",
            file=sys.stderr,
        )

    # Pairwise byte-compare.
    n = len(traces)
    total_pairs = n * (n - 1) // 2
    matches = 0
    first_div: dict[str, object] | None = None
    for i in range(n):
        for j in range(i + 1, n):
            if traces[i] == traces[j]:
                matches += 1
            elif first_div is None:
                diff = first_line_diff(traces[i], traces[j])
                first_div = {
                    "run_a": str(trace_paths[i].name),
                    "run_b": str(trace_paths[j].name),
                    "first_diverging_line": diff[0] if diff else None,
                    "a_line": diff[1] if diff else None,
                    "b_line": diff[2] if diff else None,
                }

    rate = matches / total_pairs if total_pairs > 0 else 1.0
    summary = {
        "stack": "corvid",
        "n": n,
        "byte_identical_pairs": matches,
        "total_pairs": total_pairs,
        "determinism_rate": rate,
        "first_diverging_pair": first_div,
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(
        json.dumps(summary, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    print(
        f"[corvid] done: {matches}/{total_pairs} pairs byte-identical "
        f"(rate = {rate:.3f})",
        file=sys.stderr,
    )

    return 0 if first_div is None else 1


if __name__ == "__main__":
    raise SystemExit(main())
