"""Corvid audit query Q04 — denied refunds with LLM rationale."""
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
    decision = next((e for e in events if e["kind"] == "llm_result" and e["prompt"] == "decide_refund"), None)
    user_id = events[2]["args"][0]["user_id"] if len(events) > 2 else None
    order_id = events[2]["args"][0]["order_id"] if len(events) > 2 else None
    if decision and decision["result"]["should_refund"] is False:
        records.append({"order_id": order_id, "user_id": user_id, "reason": decision["result"]["reason"]})
records.sort(key=lambda r: r["order_id"])
args.out.write_text(json.dumps(records, indent=2, sort_keys=True) + "\n")
