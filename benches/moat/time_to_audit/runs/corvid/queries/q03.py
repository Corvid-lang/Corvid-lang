"""Corvid audit query Q03 — count refunds per user."""
import argparse, json, pathlib

# region: cli-plumbing
parser = argparse.ArgumentParser()
parser.add_argument("--corpus", required=True, type=pathlib.Path)
parser.add_argument("--out", required=True, type=pathlib.Path)
args = parser.parse_args()
# endregion

counts: dict[str, int] = {}
for trace in sorted(args.corpus.glob("*.jsonl")):
    events = [json.loads(line) for line in trace.read_text().splitlines() if line]
    completed = next((e for e in events if e["kind"] == "run_completed"), None)
    refund = next((e for e in events if e["kind"] == "tool_call" and e["tool"] == "issue_refund"), None)
    user_id = events[2]["args"][0]["user_id"] if len(events) > 2 else None
    if completed and completed["ok"] and refund and user_id is not None:
        counts[user_id] = counts.get(user_id, 0) + 1
args.out.write_text(json.dumps(dict(sorted(counts.items())), indent=2, sort_keys=True) + "\n")
