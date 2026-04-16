import argparse
import datetime as dt
import json
import os
import pathlib
import subprocess
import tempfile
import time


SCENARIOS = [
    "baseline_control",
    "tool_loop",
    "retry_workflow",
    "approval_workflow",
    "replay_trace",
]

def corvid_cmd(repo: pathlib.Path) -> list[str]:
    manifest = repo / "benches" / "corvid" / "runner" / "Cargo.toml"
    tools_manifest = repo / "benches" / "corvid" / "tools" / "Cargo.toml"
    subprocess.run(
        ["cargo", "build", "--manifest-path", str(tools_manifest), "--release"],
        cwd=repo,
        check=True,
    )
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

def run_stack(repo: pathlib.Path, stack: str, base_cmd: list[str], scenario: str, profile: bool):
    fixture = repo / "benchmarks" / "cases" / f"{scenario}.json"
    with tempfile.TemporaryDirectory() as tmp:
        output = pathlib.Path(tmp) / "trial.jsonl"
        cmd = base_cmd + [str(fixture), "1", str(output)]
        env = os.environ.copy()
        if profile and stack == "corvid":
            env["CORVID_BENCH_PROFILE"] = "1"
        start = time.perf_counter()
        subprocess.run(cmd, cwd=repo, check=True, env=env)
        launcher_wall_ms = (time.perf_counter() - start) * 1000.0
        if not output.exists():
            raise FileNotFoundError(f"{stack} runner did not write {output}")
        record = json.loads(output.read_text(encoding="utf-8").strip())
    record["stack"] = stack
    record["wall_ms"] = record["total_wall_ms"]
    record["orchestration_ms"] = record["orchestration_overhead_ms"]
    record["launcher_wall_ms"] = launcher_wall_ms
    record["launcher_overhead_ms"] = launcher_wall_ms - record["total_wall_ms"]
    return record


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output-dir", required=True)
    parser.add_argument("--warmup", type=int, default=3)
    parser.add_argument("--trials", type=int, default=30)
    parser.add_argument("--profile", action="store_true")
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
                run_stack(repo, stack, cmd, scenario, args.profile)
        for trial_idx in range(1, args.trials + 1):
            for stack, cmd in stack_cmds:
                record = run_stack(repo, stack, cmd, scenario, args.profile)
                records.append(
                    {
                        "scenario": scenario,
                        "stack": stack,
                        "trial_idx": trial_idx,
                        "session_id": session_id,
                        "timestamp": dt.datetime.utcnow().isoformat() + "Z",
                        **record,
                    }
                )

    raw_path.write_text("\n".join(json.dumps(r) for r in records) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
