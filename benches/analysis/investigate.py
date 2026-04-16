import argparse
import datetime as dt
import json
import pathlib
import statistics


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


def mean(values):
    if not values:
        return 0.0
    return statistics.mean(values)


def cv_pct(values):
    if len(values) < 2:
        return 0.0
    avg = statistics.mean(values)
    if avg == 0:
        return 0.0
    return statistics.stdev(values) / avg * 100.0


def per_stack(records):
    out = {}
    for record in records:
        out.setdefault(record["stack"], []).append(record)
    return out


def med(stack_records, field):
    values = [r[field] for r in stack_records if field in r and r[field] is not None]
    return median(values)


def build_summary(records):
    by_scenario = {}
    for record in records:
        by_scenario.setdefault(record["scenario"], []).append(record)

    control = per_stack(by_scenario["baseline_control"])
    control_summary = {
        stack: {
            "median_orchestration_ms": med(stack_records, "orchestration_ms"),
            "median_launcher_overhead_ms": med(stack_records, "launcher_overhead_ms"),
            "cv_pct": cv_pct([r["orchestration_ms"] for r in stack_records]),
        }
        for stack, stack_records in control.items()
    }

    scenarios = {}
    corvid_baseline = control_summary["corvid"]["median_orchestration_ms"]

    for scenario, scenario_records in by_scenario.items():
        if scenario == "baseline_control":
            continue
        stacks = per_stack(scenario_records)
        scenario_summary = {
            stack: {
                "median_orchestration_ms": med(stack_records, "orchestration_ms"),
                "median_launcher_overhead_ms": med(stack_records, "launcher_overhead_ms"),
                "median_actual_external_wait_ms": med(stack_records, "actual_external_wait_ms"),
                "median_external_wait_bias_ms": med(stack_records, "external_wait_bias_ms"),
            }
            for stack, stack_records in stacks.items()
        }

        corvid = stacks["corvid"]
        scenario_summary["corvid"].update(
            {
                "median_runner_total_wall_ms": med(corvid, "runner_total_wall_ms"),
                "median_compile_to_ir_ms": med(corvid, "compile_to_ir_ms"),
                "median_cache_resolve_ms": med(corvid, "cache_resolve_ms"),
                "median_binary_exec_ms": med(corvid, "binary_exec_ms"),
                "median_retry_sleep_actual_ms": med(corvid, "retry_sleep_actual_ms"),
                "median_prompt_wait_actual_ms": med(corvid, "prompt_wait_actual_ms"),
                "median_tool_wait_actual_ms": med(corvid, "tool_wait_actual_ms"),
                "median_allocs": med(corvid, "allocs"),
                "median_releases": med(corvid, "releases"),
                "median_retain_calls": med(corvid, "retain_calls"),
                "median_release_calls": med(corvid, "release_calls"),
                "median_gc_trigger_count": med(corvid, "gc_trigger_count"),
                "median_safepoint_count": med(corvid, "safepoint_count"),
                "median_stack_map_entry_count": med(corvid, "stack_map_entry_count"),
                "median_verify_drift_count": med(corvid, "verify_drift_count"),
                "startup_proxy_share_pct": (
                    (corvid_baseline / scenario_summary["corvid"]["median_orchestration_ms"]) * 100.0
                    if scenario_summary["corvid"]["median_orchestration_ms"] > 0
                    else 0.0
                ),
                "retain_calls_per_step": (
                    mean(
                        [
                            r["retain_calls"] / r["logical_steps_recorded"]
                            for r in corvid
                            if r.get("retain_calls") is not None and r.get("logical_steps_recorded", 0) > 0
                        ]
                    )
                ),
                "release_calls_per_step": (
                    mean(
                        [
                            r["release_calls"] / r["logical_steps_recorded"]
                            for r in corvid
                            if r.get("release_calls") is not None and r.get("logical_steps_recorded", 0) > 0
                        ]
                    )
                ),
            }
        )

        scenario_summary["comparative"] = {
            "corvid_vs_python_median_ratio": (
                scenario_summary["corvid"]["median_orchestration_ms"]
                / scenario_summary["python"]["median_orchestration_ms"]
            ),
            "corvid_vs_typescript_median_ratio": (
                scenario_summary["corvid"]["median_orchestration_ms"]
                / scenario_summary["typescript"]["median_orchestration_ms"]
            ),
        }
        scenarios[scenario] = scenario_summary

    return {
        "generated_at": dt.datetime.utcnow().isoformat() + "Z",
        "control": control_summary,
        "scenarios": scenarios,
    }


def render_markdown(summary):
    lines = ["# Orchestration cost investigation", ""]
    lines.append(f"- Generated: `{summary['generated_at']}`")
    lines.append("")
    lines.append("## Control baseline")
    lines.append("")
    lines.append("| Stack | Median orchestration ms | Median launcher overhead ms | Control CV % |")
    lines.append("|---|---:|---:|---:|")
    for stack, data in summary["control"].items():
        lines.append(
            f"| `{stack}` | `{data['median_orchestration_ms']:.3f}` | `{data['median_launcher_overhead_ms']:.3f}` | `{data['cv_pct']:.2f}` |"
        )
    lines.append("")
    lines.append("## Scenario summaries")
    lines.append("")
    for scenario, data in summary["scenarios"].items():
        lines.append(f"### `{scenario}`")
        lines.append("")
        lines.append(
            f"- Corvid/Python median ratio: `{data['comparative']['corvid_vs_python_median_ratio']:.3f}`"
        )
        lines.append(
            f"- Corvid/TypeScript median ratio: `{data['comparative']['corvid_vs_typescript_median_ratio']:.3f}`"
        )
        lines.append(
            f"- Corvid startup proxy share of measured orchestration: `{data['corvid']['startup_proxy_share_pct']:.1f}%`"
        )
        lines.append(
            f"- Corvid median retain/release calls per logical step: `{data['corvid']['retain_calls_per_step']:.2f}` / `{data['corvid']['release_calls_per_step']:.2f}`"
        )
        lines.append(
            f"- Corvid median GC triggers / safepoints / stack-map entries: `{data['corvid']['median_gc_trigger_count']:.0f}` / `{data['corvid']['median_safepoint_count']:.0f}` / `{data['corvid']['median_stack_map_entry_count']:.0f}`"
        )
        lines.append("")
        lines.append("| Stack | Median orchestration ms | Median launcher overhead ms | Median actual external wait ms | Median wait bias ms |")
        lines.append("|---|---:|---:|---:|---:|")
        for stack in ["corvid", "python", "typescript"]:
            stack_data = data[stack]
            lines.append(
                f"| `{stack}` | `{stack_data['median_orchestration_ms']:.3f}` | `{stack_data['median_launcher_overhead_ms']:.3f}` | `{stack_data['median_actual_external_wait_ms']:.3f}` | `{stack_data['median_external_wait_bias_ms']:.3f}` |"
            )
        lines.append("")
        lines.append("| Corvid component | Median ms |")
        lines.append("|---|---:|")
        for label, field in [
            ("compile_to_ir", "median_compile_to_ir_ms"),
            ("cache_resolve", "median_cache_resolve_ms"),
            ("binary_exec", "median_binary_exec_ms"),
            ("runner_total_wall", "median_runner_total_wall_ms"),
            ("prompt_wait_actual", "median_prompt_wait_actual_ms"),
            ("tool_wait_actual", "median_tool_wait_actual_ms"),
        ]:
            lines.append(f"| `{label}` | `{data['corvid'][field]:.3f}` |")
        lines.append("")
    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("raw_jsonl")
    parser.add_argument("summary_json")
    parser.add_argument("summary_md")
    args = parser.parse_args()

    records = load_records(args.raw_jsonl)
    summary = build_summary(records)
    pathlib.Path(args.summary_json).write_text(json.dumps(summary, indent=2), encoding="utf-8")
    pathlib.Path(args.summary_md).write_text(render_markdown(summary), encoding="utf-8")


if __name__ == "__main__":
    main()
