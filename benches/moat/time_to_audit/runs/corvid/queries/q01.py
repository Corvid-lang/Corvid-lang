"""Corvid audit query Q01 — list every refund issued."""
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
    completed = next((e for e in events if e["kind"] == "run_completed"), None)
    refund = next((e for e in events if e["kind"] == "tool_call" and e["tool"] == "issue_refund"), None)
    refund_result = next((e for e in events if e["kind"] == "tool_result" and e["tool"] == "issue_refund"), None)
    decision = next((e for e in events if e["kind"] == "llm_result" and e["prompt"] == "decide_refund"), None)
    user_id = events[2]["args"][0]["user_id"] if len(events) > 2 else None
    if completed and completed["ok"] and refund and refund_result and decision:
        records.append({"order_id": refund["args"][0], "user_id": user_id, "amount": refund["args"][1],
                        "refund_id": refund_result["result"]["refund_id"], "llm_rationale": decision["result"]["reason"]})
records.sort(key=lambda r: r["order_id"])
args.out.write_text(json.dumps(records, indent=2, sort_keys=True) + "\n")
