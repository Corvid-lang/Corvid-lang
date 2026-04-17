import argparse
import json
import pathlib
import statistics
from collections import defaultdict


def load_records(path):
    return [json.loads(line) for line in pathlib.Path(path).read_text(encoding="utf-8").splitlines() if line.strip()]


def median(values):
    return statistics.median(values) if values else 0.0


def summarize(records):
    alloc_groups = defaultdict(list)
    trigger_groups = defaultdict(list)
    cycle_groups = defaultdict(list)

    for record in records:
        kind = record["kind"]
        if kind == "allocation_scaling":
            alloc_groups[record["target_release_scale"]].append(record)
        elif kind == "gc_trigger_sensitivity":
            trigger_groups[record["gc_trigger_threshold"]].append(record)
        elif kind == "cycle_stress":
            cycle_groups[record["cycle_pairs"]].append(record)

    allocation_scaling = []
    ownership_scaling = []
    for scale in sorted(alloc_groups):
        items = alloc_groups[scale]
        orchestration_ms = median([item["orchestration_ms"] for item in items])
        gc_total_ms = median([item["gc_total_ms"] for item in items])
        entry = {
            "target_release_scale": scale,
            "median_orchestration_ms": orchestration_ms,
            "median_gc_total_ms": gc_total_ms,
            "gc_pct_of_orchestration": (gc_total_ms / orchestration_ms * 100.0) if orchestration_ms else 0.0,
            "median_rc_release_count": median([item["rc_release_count"] for item in items]),
            "median_gc_mark_count": median([item["gc_mark_count"] for item in items]),
            "median_gc_sweep_count": median([item["gc_sweep_count"] for item in items]),
            "median_peak_live_objects": median([item["peak_live_objects"] for item in items]),
        }
        allocation_scaling.append(entry)
        ownership_scaling.append(
            {
                "target_release_scale": scale,
                "median_rc_retain_count": median([item["rc_retain_count"] for item in items]),
                "median_rc_release_count": median([item["rc_release_count"] for item in items]),
            }
        )

    trigger_sensitivity = []
    for threshold in sorted(trigger_groups):
        items = trigger_groups[threshold]
        orchestration_ms = median([item["orchestration_ms"] for item in items])
        gc_total_ms = median([item["gc_total_ms"] for item in items])
        trigger_sensitivity.append(
            {
                "gc_trigger_threshold": threshold,
                "median_orchestration_ms": orchestration_ms,
                "median_gc_total_ms": gc_total_ms,
                "gc_pct_of_orchestration": (gc_total_ms / orchestration_ms * 100.0) if orchestration_ms else 0.0,
                "median_gc_trigger_count": median([item["gc_trigger_count"] for item in items]),
                "median_peak_live_objects": median([item["peak_live_objects"] for item in items]),
            }
        )

    cycle_stress = []
    for cycle_pairs in sorted(cycle_groups):
        items = cycle_groups[cycle_pairs]
        orchestration_ms = median([item["orchestration_ms"] for item in items])
        gc_total_ms = median([item["gc_total_ms"] for item in items])
        cycle_stress.append(
            {
                "cycle_pairs": cycle_pairs,
                "median_orchestration_ms": orchestration_ms,
                "median_gc_total_ms": gc_total_ms,
                "gc_pct_of_orchestration": (gc_total_ms / orchestration_ms * 100.0) if orchestration_ms else 0.0,
                "median_gc_cycle_count": median([item["gc_cycle_count"] for item in items]),
                "median_gc_mark_count": median([item["gc_mark_count"] for item in items]),
                "median_gc_sweep_count": median([item["gc_sweep_count"] for item in items]),
                "median_peak_live_objects": median([item["peak_live_objects"] for item in items]),
            }
        )

    return {
        "allocation_scaling": allocation_scaling,
        "trigger_sensitivity": trigger_sensitivity,
        "cycle_stress": cycle_stress,
        "ownership_scaling": ownership_scaling,
    }


def render_markdown(summary):
    lines = [
        "# RC/GC tuning assessment",
        "",
        "## Allocation-pressure scaling",
        "",
        "| Target releases / trial | Median orchestration ms | Median GC ms | GC % of orchestration | Median mark count | Median sweep count | Median peak live objects |",
        "| --- | ---: | ---: | ---: | ---: | ---: | ---: |",
    ]
    for row in summary["allocation_scaling"]:
        lines.append(
            f"| `{int(row['target_release_scale'])}` | `{row['median_orchestration_ms']:.6f}` | `{row['median_gc_total_ms']:.6f}` | `{row['gc_pct_of_orchestration']:.1f}%` | `{int(row['median_gc_mark_count'])}` | `{int(row['median_gc_sweep_count'])}` | `{int(row['median_peak_live_objects'])}` |"
        )

    lines.extend(
        [
            "",
            "## GC trigger sensitivity",
            "",
            "| GC cadence | Median orchestration ms | Median GC ms | GC % of orchestration | Median GC count | Median peak live objects |",
            "| --- | ---: | ---: | ---: | ---: | ---: |",
        ]
    )
    for row in summary["trigger_sensitivity"]:
        label = "disabled" if row["gc_trigger_threshold"] == 0 else int(row["gc_trigger_threshold"])
        lines.append(
            f"| `{label}` | `{row['median_orchestration_ms']:.6f}` | `{row['median_gc_total_ms']:.6f}` | `{row['gc_pct_of_orchestration']:.1f}%` | `{int(row['median_gc_trigger_count'])}` | `{int(row['median_peak_live_objects'])}` |"
        )

    lines.extend(
        [
            "",
            "## Cycle collector stress",
            "",
            "| Cycle pairs / trial | Median orchestration ms | Median GC ms | GC % of orchestration | Median reclaimed cycle objects | Median sweep count | Median peak live objects |",
            "| --- | ---: | ---: | ---: | ---: | ---: | ---: |",
        ]
    )
    for row in summary["cycle_stress"]:
        lines.append(
            f"| `{int(row['cycle_pairs'])}` | `{row['median_orchestration_ms']:.6f}` | `{row['median_gc_total_ms']:.6f}` | `{row['gc_pct_of_orchestration']:.1f}%` | `{int(row['median_gc_cycle_count'])}` | `{int(row['median_gc_sweep_count'])}` | `{int(row['median_peak_live_objects'])}` |"
        )

    lines.extend(
        [
            "",
            "## Ownership pass at scale",
            "",
            "| Target releases / trial | Median retain count | Median release count |",
            "| --- | ---: | ---: |",
        ]
    )
    for row in summary["ownership_scaling"]:
        lines.append(
            f"| `{int(row['target_release_scale'])}` | `{int(row['median_rc_retain_count'])}` | `{int(row['median_rc_release_count'])}` |"
        )

    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("raw_jsonl")
    parser.add_argument("summary_json")
    parser.add_argument("summary_md")
    args = parser.parse_args()

    records = load_records(args.raw_jsonl)
    summary = summarize(records)
    pathlib.Path(args.summary_json).write_text(json.dumps(summary, indent=2), encoding="utf-8")
    pathlib.Path(args.summary_md).write_text(render_markdown(summary), encoding="utf-8")


if __name__ == "__main__":
    main()
