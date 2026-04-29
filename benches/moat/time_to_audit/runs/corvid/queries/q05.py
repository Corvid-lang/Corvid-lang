"""Corvid audit query Q05 — approval tokens issued."""
import argparse, json, pathlib

# region: cli-plumbing
parser = argparse.ArgumentParser()
parser.add_argument("--corpus", required=True, type=pathlib.Path)
parser.add_argument("--out", required=True, type=pathlib.Path)
args = parser.parse_args()
# endregion

records = []
for trace in sorted(args.corpus.glob("*.jsonl")):
    events = [json.loads(line) for line in trace.read_text().splitlines() if line]
    user_id = events[2]["args"][0]["user_id"] if len(events) > 2 else None
    for e in events:
        if e["kind"] == "approval_token_issued":
            records.append({"label": e["label"], "args": e["args"], "scope": e["scope"],
                            "order_id": e["args"][0], "user_id": user_id})
records.sort(key=lambda r: r["order_id"])
args.out.write_text(json.dumps(records, indent=2, sort_keys=True) + "\n")
