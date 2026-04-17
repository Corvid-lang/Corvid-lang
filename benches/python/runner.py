import argparse
import json
import pathlib
import time


def run_trial(fixture: dict, trial: int) -> dict:
    events = []
    state = {"outputs": {}}
    start = time.perf_counter()
    actual_external_wait_ms = 0.0
    for step in fixture["steps"]:
        latency_ms = step.get("external_latency_ms", 0)
        step_kind = step["kind"]
        step_name = step["name"]

        if step_kind == "prompt":
            rendered = json.dumps(step.get("inputs", {}), separators=(",", ":"), sort_keys=True)
            response = json.loads(step["mock_response"])
            state["outputs"][step_name] = {"rendered": rendered, "response": response}
        elif step_kind == "tool":
            request = json.dumps(step.get("inputs", {}), separators=(",", ":"), sort_keys=True)
            response = json.loads(json.dumps(step.get("mock_output")))
            state["outputs"][step_name] = {"request": request, "response": response}
        elif step_kind == "approval":
            proposal = json.dumps(step.get("inputs", {}), separators=(",", ":"), sort_keys=True)
            decision = step.get("approval_outcome", "granted")
            state["outputs"][step_name] = {"proposal": proposal, "decision": decision}
        elif step_kind == "retry_sleep":
            state["outputs"][step_name] = {"sleep_ms": latency_ms}
        elif step_kind == "replay_checkpoint":
            state["outputs"][step_name] = {"checkpoint": step.get("mock_output")}

        if latency_ms:
            wait_start = time.perf_counter()
            time.sleep(latency_ms / 1000.0)
            actual_external_wait_ms += (time.perf_counter() - wait_start) * 1000.0
        events.append(f"{step_kind}:{step_name}")
    elapsed_ms = (time.perf_counter() - start) * 1000.0
    external_wait_ms = sum(step.get("external_latency_ms", 0) for step in fixture["steps"])
    final_output = fixture.get("expected_final_output")
    trace_bytes = len("\n".join(events).encode("utf-8"))
    return {
        "implementation": "python-asyncio-stdlib",
        "process_mode": "persistent",
        "fixture": fixture["name"],
        "trial": trial,
        "success": True,
        "stdout_match": final_output == fixture.get("expected_final_output"),
        "total_wall_ms": elapsed_ms,
        "external_wait_ms": external_wait_ms,
        "actual_external_wait_ms": actual_external_wait_ms,
        "external_wait_bias_ms": actual_external_wait_ms - external_wait_ms,
        "orchestration_overhead_ms": elapsed_ms - actual_external_wait_ms,
        "trace_size_raw_bytes": trace_bytes,
        "logical_steps_recorded": len(events),
        "bytes_per_step": trace_bytes / len(events) if events else 0.0,
        "replay_supported": False,
        "expected_replay_steps": len(fixture.get("expected_replay_events", [])),
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("fixture")
    parser.add_argument("trials", type=int, nargs="?")
    parser.add_argument("output", nargs="?")
    parser.add_argument("--server", action="store_true")
    args = parser.parse_args()

    fixture = json.loads(pathlib.Path(args.fixture).read_text(encoding="utf-8"))
    if args.server:
        import sys

        for line in sys.stdin:
            line = line.strip()
            if not line:
                continue
            request = json.loads(line)
            trial = int(request["trial_idx"])
            print(json.dumps(run_trial(fixture, trial)), flush=True)
        return

    if args.trials is None or args.output is None:
        raise SystemExit("usage: runner.py <fixture.json> <trials> <output.jsonl> | runner.py --server <fixture.json>")

    output_path = pathlib.Path(args.output)
    output_path.parent.mkdir(parents=True, exist_ok=True)
    with output_path.open("w", encoding="utf-8") as f:
        for trial in range(1, args.trials + 1):
            f.write(json.dumps(run_trial(fixture, trial)))
            f.write("\n")


if __name__ == "__main__":
    main()
