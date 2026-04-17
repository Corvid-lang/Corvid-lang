import argparse
import datetime as dt
import json
import pathlib
import statistics


COMPONENT_FIELDS = [
    ("prompt_render_ms", "prompt_render"),
    ("json_bridge_ms", "json_bridge"),
    ("mock_llm_dispatch_ms", "mock_llm_dispatch"),
    ("trial_init_ms", "trial_init"),
    ("trace_overhead_ms", "trace_overhead"),
    ("rc_release_time_ms", "rc_release_time"),
    ("unattributed_ms", "unattributed"),
]


def load_records(path: str):
    return [
        json.loads(line)
        for line in pathlib.Path(path).read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]


def median(values):
    if not values:
        return 0.0
    return statistics.median(values)


def percentile(values, q):
    if not values:
        return 0.0
    ordered = sorted(values)
    if len(ordered) == 1:
        return ordered[0]
    pos = (len(ordered) - 1) * q
    low = int(pos)
    high = min(low + 1, len(ordered) - 1)
    weight = pos - low
    return ordered[low] * (1 - weight) + ordered[high] * weight


def iqr(values):
    return percentile(values, 0.25), percentile(values, 0.75)


def corvid_by_scenario(records):
    scenarios = {}
    for record in records:
        if record.get("stack") != "corvid":
            continue
        scenarios.setdefault(record["scenario"], []).append(record)
    return scenarios


def control_summary(records):
    scenarios = corvid_by_scenario(records)
    values = [record["orchestration_ms"] for record in scenarios.get("baseline_control", [])]
    q1, q3 = iqr(values)
    return {
        "median_orchestration_ms": median(values),
        "iqr_low_ms": q1,
        "iqr_high_ms": q3,
    }


def scenario_breakdowns(records):
    scenarios = corvid_by_scenario(records)
    out = {}
    for scenario, scenario_records in scenarios.items():
        if scenario == "baseline_control":
            continue
        orchestration = median([record["orchestration_ms"] for record in scenario_records])
        components = []
        for field, label in COMPONENT_FIELDS:
            value = median([record.get(field, 0.0) for record in scenario_records])
            pct = (value / orchestration * 100.0) if orchestration > 0 else 0.0
            components.append(
                {
                    "field": field,
                    "label": label,
                    "median_ms": value,
                    "pct_of_orchestration": pct,
                }
            )
        out[scenario] = {
            "median_orchestration_ms": orchestration,
            "components": components,
            "total_profiled_ms": median(
                [record.get("total_profiled_ms", 0.0) for record in scenario_records]
            ),
        }
    return out


def trace_deltas(profile_records, trace_records):
    profile = scenario_breakdowns(profile_records)
    trace = scenario_breakdowns(trace_records)
    out = {}
    for scenario, profile_data in profile.items():
        trace_data = trace.get(scenario)
        if not trace_data:
            continue
        base = profile_data["median_orchestration_ms"]
        delta = trace_data["median_orchestration_ms"] - base
        pct = (delta / base * 100.0) if base > 0 else 0.0
        out[scenario] = {
            "trace_off_median_ms": base,
            "trace_on_median_ms": trace_data["median_orchestration_ms"],
            "delta_ms": delta,
            "delta_pct": pct,
        }
    return out


def profiling_overhead(profile_records, control_records):
    profile = scenario_breakdowns(profile_records)
    control = scenario_breakdowns(control_records)
    out = {}
    for scenario, profile_data in profile.items():
        control_data = control.get(scenario)
        if not control_data:
            continue
        base = control_data["median_orchestration_ms"]
        delta = profile_data["median_orchestration_ms"] - base
        pct = (delta / base * 100.0) if base > 0 else 0.0
        out[scenario] = {
            "control_median_ms": base,
            "profile_median_ms": profile_data["median_orchestration_ms"],
            "delta_ms": delta,
            "delta_pct": pct,
        }
    return out


def build_summary(profile_records, trace_records, control_records):
    return {
        "generated_at": dt.datetime.utcnow().isoformat() + "Z",
        "profile_control": control_summary(profile_records),
        "trace_control": control_summary(trace_records),
        "control_session_control": control_summary(control_records),
        "scenarios": scenario_breakdowns(profile_records),
        "trace_deltas": trace_deltas(profile_records, trace_records),
        "profiling_overhead_ab": profiling_overhead(profile_records, control_records),
    }


def render_markdown(summary):
    lines = ["# Residual orchestration cost breakdown", ""]
    lines.append(f"- Generated: `{summary['generated_at']}`")
    lines.append("- Corvid-only residual breakdown using the instrumented persistent runner")
    lines.append("")
    lines.append("## Control disclosure")
    lines.append("")
    lines.append(
        "Absolute control medians are more informative than coefficient of variation here because the control mean is near zero."
    )
    lines.append("")
    lines.append("| Session | Control median ms | IQR |")
    lines.append("|---|---:|---:|")
    for label, key in [
        ("profile", "profile_control"),
        ("trace-on", "trace_control"),
        ("control", "control_session_control"),
    ]:
        data = summary[key]
        lines.append(
            f"| `{label}` | `{data['median_orchestration_ms']:.6f}` | `[{data['iqr_low_ms']:.6f}, {data['iqr_high_ms']:.6f}]` |"
        )
    lines.append("")
    lines.append("## Attribution rule")
    lines.append("")
    lines.append("- `prompt_render`: runtime string helper time used by prompt assembly")
    lines.append(
        "- `json_bridge`: prompt bridge overhead after subtracting measured wait and mock dispatch"
    )
    lines.append("- `mock_llm_dispatch`: mock lookup and reply construction, excluding sleep")
    lines.append("- `trial_init`: per-trial reset/setup inside the persistent native entry loop")
    lines.append("- `trace_overhead`: direct trace emit counter inside the runtime")
    lines.append("- `rc_release_time`: time spent inside `corvid_release`")
    lines.append(
        "- `unattributed`: `orchestration_ms - sum(profiled components)` at the per-trial record level"
    )
    lines.append("")
    lines.append("## Scenario breakdown")
    lines.append("")
    for scenario, data in summary["scenarios"].items():
        lines.append(f"### `{scenario}`")
        lines.append("")
        lines.append(f"- Corvid median orchestration: `{data['median_orchestration_ms']:.6f} ms`")
        lines.append(f"- Profiled total median: `{data['total_profiled_ms']:.6f} ms`")
        lines.append("")
        lines.append("| Component | Median ms | % of orchestration |")
        lines.append("|---|---:|---:|")
        for component in data["components"]:
            lines.append(
                f"| `{component['label']}` | `{component['median_ms']:.6f}` | `{component['pct_of_orchestration']:.1f}%` |"
            )
        lines.append("")
        trace_delta = summary["trace_deltas"].get(scenario)
        if trace_delta:
            lines.append(
                f"- Trace-on delta vs trace-off: `{trace_delta['delta_ms']:+.6f} ms` (`{trace_delta['delta_pct']:+.2f}%`)"
            )
        profile_delta = summary["profiling_overhead_ab"].get(scenario)
        if profile_delta:
            lines.append(
                f"- Profile-session delta vs same-tree control: `{profile_delta['delta_ms']:+.6f} ms` (`{profile_delta['delta_pct']:+.2f}%`)"
            )
        lines.append("")
    lines.append("## Instrumentation note")
    lines.append("")
    lines.append(
        "The profile-vs-control A/B did not produce a stable overhead estimate. Session-to-session noise on the host was larger than the expected timer tax, so the results are reported as inconclusive rather than proving a <5% measurement overhead."
    )
    lines.append("")
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("profile_raw_jsonl")
    parser.add_argument("trace_raw_jsonl")
    parser.add_argument("control_raw_jsonl")
    parser.add_argument("summary_json")
    parser.add_argument("summary_md")
    args = parser.parse_args()

    summary = build_summary(
        load_records(args.profile_raw_jsonl),
        load_records(args.trace_raw_jsonl),
        load_records(args.control_raw_jsonl),
    )
    pathlib.Path(args.summary_json).write_text(json.dumps(summary, indent=2), encoding="utf-8")
    pathlib.Path(args.summary_md).write_text(render_markdown(summary), encoding="utf-8")


if __name__ == "__main__":
    main()
