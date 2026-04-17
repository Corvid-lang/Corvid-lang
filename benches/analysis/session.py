import argparse
import datetime as dt
import json
import os
import pathlib
import subprocess


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


class RunnerServer:
    def __init__(
        self,
        repo: pathlib.Path,
        stack: str,
        base_cmd: list[str],
        scenario: str,
        total_requests: int,
        profile: bool,
    ) -> None:
        fixture = repo / "benchmarks" / "cases" / f"{scenario}.json"
        cmd = list(base_cmd)
        if stack == "corvid":
            cmd += ["--server", str(fixture), str(total_requests)]
        else:
            cmd += ["--server", str(fixture)]
        env = os.environ.copy()
        if profile and stack == "corvid":
            env["CORVID_BENCH_PROFILE"] = "1"
        self.proc = subprocess.Popen(
            cmd,
            cwd=repo,
            env=env,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            bufsize=1,
        )
        self.stack = stack

    def run_trial(self, trial_idx: int) -> dict:
        assert self.proc.stdin is not None
        assert self.proc.stdout is not None
        payload = json.dumps({"trial_idx": trial_idx})
        self.proc.stdin.write(payload + "\n")
        self.proc.stdin.flush()
        line = self.proc.stdout.readline()
        if not line:
            stderr = ""
            if self.proc.stderr is not None:
                stderr = self.proc.stderr.read()
            raise RuntimeError(f"{self.stack} runner ended early: {stderr}")
        record = json.loads(line)
        record["stack"] = self.stack
        record["wall_ms"] = record["total_wall_ms"]
        record["orchestration_ms"] = record["orchestration_overhead_ms"]
        record.setdefault("launcher_wall_ms", 0.0)
        record.setdefault("launcher_overhead_ms", 0.0)
        return record

    def close(self) -> None:
        if self.proc.stdin is not None:
            self.proc.stdin.close()
        stderr = ""
        if self.proc.stderr is not None:
            stderr = self.proc.stderr.read()
        rc = self.proc.wait()
        if rc != 0:
            raise RuntimeError(f"{self.stack} runner exited with {rc}: {stderr}")


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
    total_requests = args.warmup + args.trials
    for scenario in SCENARIOS:
        servers = [
            RunnerServer(repo, stack, cmd, scenario, total_requests, args.profile)
            for stack, cmd in stack_cmds
        ]
        try:
            for warm_idx in range(1, args.warmup + 1):
                for server in servers:
                    server.run_trial(warm_idx)
            for trial_idx in range(1, args.trials + 1):
                request_idx = args.warmup + trial_idx
                for server in servers:
                    record = server.run_trial(request_idx)
                    records.append(
                        {
                            "scenario": scenario,
                            "stack": server.stack,
                            "trial_idx": trial_idx,
                            "session_id": session_id,
                            "timestamp": dt.datetime.utcnow().isoformat() + "Z",
                            **record,
                        }
                    )
        finally:
            for server in servers:
                server.close()

    raw_path.write_text("\n".join(json.dumps(r) for r in records) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
