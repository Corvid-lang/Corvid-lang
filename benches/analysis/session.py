import argparse
import datetime as dt
import json
import os
import pathlib
import subprocess
import tempfile


SCENARIOS = [
    "baseline_control",
    "tool_loop",
    "retry_workflow",
    "approval_workflow",
    "replay_trace",
]

def corvid_cmd(repo: pathlib.Path) -> list[str]:
    manifest = repo / "benches" / "corvid" / "runner" / "Cargo.toml"
    subprocess.run(
        ["cargo", "build", "--manifest-path", str(manifest)],
        cwd=repo,
        check=True,
    )
    exe = repo / "benches" / "corvid" / "runner" / "target" / "debug" / (
        "corvid_bench_runner.exe" if os.name == "nt" else "corvid_bench_runner"
    )
    return [str(exe)]


def typescript_cmd(repo: pathlib.Path) -> list[str]:
    ts_dir = repo / "benches" / "typescript"
    npm = "npm.cmd" if os.name == "nt" else "npm"
    npx = "npx.cmd" if os.name == "nt" else "npx"
    if not (ts_dir / "node_modules").exists():
        subprocess.run([npm, "install"], cwd=ts_dir, check=True)
    return [npx, "tsx", "benches/typescript/runner.ts"]


def stacks(repo: pathlib.Path):
    return [
        ("corvid", corvid_cmd(repo)),
        ("python", ["python", "benches/python/runner.py"]),
        ("typescript", typescript_cmd(repo)),
    ]


def run_stack(repo: pathlib.Path, stack: str, base_cmd: list[str], scenario: str):
    fixture = repo / "benchmarks" / "cases" / f"{scenario}.json"
    with tempfile.TemporaryDirectory() as tmp:
        output = pathlib.Path(tmp) / "trial.jsonl"
        cmd = base_cmd + [str(fixture), "1", str(output)]
        subprocess.run(cmd, cwd=repo, check=True)
        record = json.loads(output.read_text(encoding="utf-8").strip())
    return {
        "stack": stack,
        "wall_ms": record["total_wall_ms"],
        "external_wait_ms": record["external_wait_ms"],
        "orchestration_ms": record["orchestration_overhead_ms"],
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output-dir", required=True)
    parser.add_argument("--warmup", type=int, default=3)
    parser.add_argument("--trials", type=int, default=30)
    args = parser.parse_args()

    repo = pathlib.Path(__file__).resolve().parents[2]
    output_dir = pathlib.Path(args.output_dir)
    output_dir.mkdir(parents=True, exist_ok=True)
    raw_path = output_dir / "raw.jsonl"
    session_id = output_dir.name
    stack_cmds = stacks(repo)

    records = []
    for scenario in SCENARIOS:
        for _ in range(args.warmup):
            for stack, cmd in stack_cmds:
                run_stack(repo, stack, cmd, scenario)
        for trial_idx in range(1, args.trials + 1):
            for stack, cmd in stack_cmds:
                record = run_stack(repo, stack, cmd, scenario)
                records.append(
                    {
                        "scenario": scenario,
                        "stack": stack,
                        "trial_idx": trial_idx,
                        "wall_ms": record["wall_ms"],
                        "external_wait_ms": record["external_wait_ms"],
                        "orchestration_ms": record["orchestration_ms"],
                        "session_id": session_id,
                        "timestamp": dt.datetime.utcnow().isoformat() + "Z",
                    }
                )

    raw_path.write_text("\n".join(json.dumps(r) for r in records) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
