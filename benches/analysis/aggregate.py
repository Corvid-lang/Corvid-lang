import argparse
import datetime as dt
import json
import math
import pathlib
import random
import statistics


def median(values):
    return statistics.median(values)


def percentile(values, q):
    if not values:
        return 0.0
    ordered = sorted(values)
    if len(ordered) == 1:
        return ordered[0]
    pos = (len(ordered) - 1) * q
    low = math.floor(pos)
    high = math.ceil(pos)
    if low == high:
        return ordered[low]
    weight = pos - low
    return ordered[low] * (1 - weight) + ordered[high] * weight


def bootstrap_ratio(left, right, resamples=10_000, seed=42):
    rng = random.Random(seed)
    n = len(left)
    stats = []
    for _ in range(resamples):
        idxs = [rng.randrange(n) for _ in range(n)]
        sample_left = [left[i] for i in idxs]
        sample_right = [right[i] for i in idxs]
        stats.append(median(sample_left) / median(sample_right))
    stats.sort()
    return percentile(stats, 0.025), percentile(stats, 0.975)


def load_records(path):
    with open(path, "r", encoding="utf-8") as f:
        return [json.loads(line) for line in f if line.strip()]


def noise_floor(control_records):
    per_stack = {}
    for record in control_records:
        per_stack.setdefault(record["stack"], []).append(record["orchestration_ms"])
    floors = {}
    for stack, values in per_stack.items():
        if len(values) > 1 and statistics.mean(values) != 0:
            floors[stack] = statistics.stdev(values) / statistics.mean(values) * 100.0
        else:
            floors[stack] = 0.0
    return {
        "per_stack_cv_pct": floors,
        "disclosed_cv_pct": max(floors.values()) if floors else 0.0,
    }


def build_summary(records):
    by_scenario = {}
    for record in records:
        by_scenario.setdefault(record["scenario"], []).append(record)

    ratios = {}
    control = by_scenario.pop("baseline_control", [])
    for scenario, scenario_records in by_scenario.items():
        stack_map = {}
        for record in scenario_records:
            stack_map.setdefault(record["stack"], {})[record["trial_idx"]] = record["orchestration_ms"]

        corvid = [stack_map["corvid"][i] for i in sorted(stack_map["corvid"])]
        python = [stack_map["python"][i] for i in sorted(stack_map["python"])]
        typescript = [stack_map["typescript"][i] for i in sorted(stack_map["typescript"])]

        cp_ratios = [c / p for c, p in zip(corvid, python)]
        ct_ratios = [c / t for c, t in zip(corvid, typescript)]

        cp_ci = bootstrap_ratio(corvid, python)
        ct_ci = bootstrap_ratio(corvid, typescript)

        ratios[scenario] = {
            "corvid_vs_python": {
                "median_ratio": median(corvid) / median(python),
                "ci95": cp_ci,
                "p50": percentile(cp_ratios, 0.50),
                "p90": percentile(cp_ratios, 0.90),
                "p99": percentile(cp_ratios, 0.99),
            },
            "corvid_vs_typescript": {
                "median_ratio": median(corvid) / median(typescript),
                "ci95": ct_ci,
                "p50": percentile(ct_ratios, 0.50),
                "p90": percentile(ct_ratios, 0.90),
                "p99": percentile(ct_ratios, 0.99),
            },
        }

    return {
        "generated_at": dt.datetime.utcnow().isoformat() + "Z",
        "noise_floor": noise_floor(control),
        "scenarios": ratios,
    }


def render_markdown(summary):
    lines = []
    lines.append("# Same-session ratios")
    lines.append("")
    lines.append(f"- Generated: `{summary['generated_at']}`")
    lines.append(
        f"- Noise floor disclosure: `{summary['noise_floor']['disclosed_cv_pct']:.2f}%` control CV (worst-stack)"
    )
    lines.append("")
    lines.append("## Scenario ratios")
    lines.append("")
    for scenario, data in summary["scenarios"].items():
        lines.append(f"### `{scenario}`")
        lines.append("")
        lines.append("| Comparison | Median ratio | 95% CI | p50 | p90 | p99 |")
        lines.append("|---|---:|---:|---:|---:|---:|")
        for label, stats in data.items():
            lo, hi = stats["ci95"]
            lines.append(
                f"| `{label}` | `{stats['median_ratio']:.3f}` | `[{lo:.3f}, {hi:.3f}]` | `{stats['p50']:.3f}` | `{stats['p90']:.3f}` | `{stats['p99']:.3f}` |"
            )
        lines.append("")
    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("raw_jsonl")
    parser.add_argument("ratios_json")
    parser.add_argument("ratios_md")
    args = parser.parse_args()

    records = load_records(args.raw_jsonl)
    summary = build_summary(records)

    pathlib.Path(args.ratios_json).write_text(json.dumps(summary, indent=2), encoding="utf-8")
    pathlib.Path(args.ratios_md).write_text(render_markdown(summary), encoding="utf-8")


if __name__ == "__main__":
    main()
