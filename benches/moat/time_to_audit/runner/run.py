"""Time-to-audit benchmark runner.

For each (stack, audit-question) pair, runs the stack's query
implementation against the stack's corpus, byte-compares the
emitted answer against the question's `expected_answer.json`,
counts LOC of the query (excluding blanks, pure-comment lines,
and the bounded `# region: cli-plumbing` block), and records
wall-clock time.

Stacks without a `runs/<stack>/queries/q<NN>.<ext>` for a given
question are reported as `bounty-open` for that question.

The drift-gated artifact is `RESULTS.md`. Each stack's per-query
verdict + total LOC + total wall-clock land in the markdown
table; CI diffs the regenerated file against the committed one.

Usage:
    python benches/moat/time_to_audit/runner/run.py \\
        --root benches/moat/time_to_audit \\
        --out  benches/moat/time_to_audit/RESULTS.md
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass, field
from pathlib import Path


STACKS = [
    ("corvid", "Corvid (JSONL trace under `target/trace/`)"),
    ("python", "Python (LangChain + LangSmith)"),
    ("typescript", "TypeScript (Vercel AI SDK + OTEL)"),
]

QUERY_EXTENSIONS = {
    "corvid": "py",
    "python": "py",
    "typescript": "ts",
}


@dataclass
class QueryResult:
    qid: str
    title: str
    stack: str
    status: str  # "ok" | "wrong-answer" | "bounty-open"
    loc: int | None = None
    wall_seconds: float | None = None
    error: str | None = None


@dataclass
class StackTotals:
    stack: str
    label: str
    queries_run: int = 0
    queries_correct: int = 0
    queries_open: int = 0
    total_loc: int = 0
    total_wall_seconds: float = 0.0


def discover_questions(root: Path) -> list[tuple[str, str]]:
    out: list[tuple[str, str]] = []
    for d in sorted((root / "audit_questions").iterdir()):
        if not d.is_dir():
            continue
        qid = d.name
        question_md = d / "question.md"
        if not question_md.exists():
            raise SystemExit(f"missing {question_md}")
        first_line = question_md.read_text(encoding="utf-8").splitlines()[0]
        title = first_line.lstrip("#").strip()
        out.append((qid, title))
    return out


def count_loc(query_path: Path) -> int:
    """Count audit-logic LOC, excluding blanks, pure-comment lines, and
    a bounded `# region: cli-plumbing` block."""
    text = query_path.read_text(encoding="utf-8")
    in_cli_region = False
    n = 0
    for raw in text.splitlines():
        line = raw.rstrip()
        stripped = line.strip()
        if stripped.startswith("# region: cli-plumbing"):
            in_cli_region = True
            continue
        if stripped.startswith("# endregion"):
            in_cli_region = False
            continue
        if in_cli_region:
            continue
        if not stripped:
            continue
        if stripped.startswith("#") or stripped.startswith("//"):
            continue
        n += 1
    return n


def run_query(
    query_path: Path,
    corpus_dir: Path,
    expected_answer: Path,
    qid: str,
    stack: str,
    title: str,
) -> QueryResult:
    if not corpus_dir.exists():
        return QueryResult(
            qid=qid,
            title=title,
            stack=stack,
            status="bounty-open",
            error=f"no corpus under {corpus_dir}",
        )
    with tempfile.TemporaryDirectory() as td:
        out_path = Path(td) / "answer.json"
        cmd = (
            ["python", str(query_path)]
            if query_path.suffix == ".py"
            else ["node", str(query_path)]
        )
        cmd += [
            "--corpus", str(corpus_dir),
            "--out", str(out_path),
        ]
        t0 = time.perf_counter()
        proc = subprocess.run(
            cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True
        )
        wall = time.perf_counter() - t0
        if proc.returncode != 0:
            return QueryResult(
                qid=qid,
                title=title,
                stack=stack,
                status="wrong-answer",
                wall_seconds=wall,
                error=f"query exited {proc.returncode}: {proc.stderr.strip()}",
            )
        if not out_path.exists():
            return QueryResult(
                qid=qid,
                title=title,
                stack=stack,
                status="wrong-answer",
                wall_seconds=wall,
                error="query did not write --out file",
            )
        actual = json.loads(out_path.read_text(encoding="utf-8"))
    expected = json.loads(expected_answer.read_text(encoding="utf-8"))
    actual_canon = json.dumps(actual, indent=2, sort_keys=True) + "\n"
    expected_canon = json.dumps(expected, indent=2, sort_keys=True) + "\n"
    if actual_canon != expected_canon:
        return QueryResult(
            qid=qid,
            title=title,
            stack=stack,
            status="wrong-answer",
            wall_seconds=wall,
            error="answer differs from expected_answer.json",
        )
    return QueryResult(
        qid=qid,
        title=title,
        stack=stack,
        status="ok",
        loc=count_loc(query_path),
        wall_seconds=wall,
    )


def render_results_md(
    questions: list[tuple[str, str]],
    per_query: list[QueryResult],
    totals: list[StackTotals],
) -> str:
    out: list[str] = []
    out.append("# Time-to-audit — published results\n")
    out.append(
        "> Auto-generated by "
        "`benches/moat/time_to_audit/runner/run.py`. "
        "Do not hand-edit. Re-run the runner after adding or "
        "modifying queries / corpus / questions.\n"
    )

    out.append("## Headline numbers\n")
    out.append(
        "Lines of audit-logic code required to answer all "
        f"{len(questions)} representative audit questions against the "
        "stack's canonical trace surface (lower is better):\n"
    )
    for t in totals:
        if t.queries_correct == len(questions):
            out.append(
                f"- {t.label}: **{t.total_loc} LOC** "
                f"(all {t.queries_correct} queries correct)"
            )
        elif t.queries_correct > 0:
            out.append(
                f"- {t.label}: {t.queries_correct}/{len(questions)} correct "
                f"({t.queries_open} bounty-open)"
            )
        else:
            out.append(
                f"- {t.label}: **bounty-open** "
                f"({t.queries_open}/{len(questions)} queries unimplemented)"
            )
    out.append("")

    out.append("## Per-query verdicts\n")
    out.append(
        "| Question | "
        + " | ".join(
            f"{label.split(' (')[0]}" for _, label in STACKS
        )
        + " |"
    )
    out.append("|" + "---|" * (1 + len(STACKS)))
    for qid, title in questions:
        cells: list[str] = [f"`{qid}`"]
        for stack, _ in STACKS:
            r = next(
                (
                    r
                    for r in per_query
                    if r.qid == qid and r.stack == stack
                ),
                None,
            )
            if r is None or r.status == "bounty-open":
                cells.append("bounty-open")
            elif r.status == "ok":
                cells.append(f"{r.loc} LOC")
            else:
                cells.append(f"WRONG ({r.error})")
        out.append("| " + " | ".join(cells) + " |")

    out.append("\n## Methodology\n")
    out.append(
        "See `benches/moat/time_to_audit/README.md` for the corpus "
        "format, what counts as LOC, the bounty rules, and how to "
        "submit a Python or TypeScript implementation. The runner "
        "validates each answer byte-for-byte against the question's "
        "`expected_answer.json` (after sorting JSON keys), so a stack "
        "that appears to win by emitting fewer fields is flagged "
        "automatically as a wrong answer.\n\n"
        "Wall-clock numbers are printed to stderr by the runner but "
        "deliberately not embedded in this file — they would break "
        "the drift gate by varying per-machine, and they are not "
        "load-bearing at this corpus size. Re-run the runner locally "
        "to see them."
    )

    return "\n".join(out) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()

    questions = discover_questions(args.root)
    per_query: list[QueryResult] = []
    totals: list[StackTotals] = []

    for stack, label in STACKS:
        ext = QUERY_EXTENSIONS[stack]
        corpus_dir = args.root / "corpus" / stack
        queries_dir = args.root / "runs" / stack / "queries"
        st = StackTotals(stack=stack, label=label)
        for qid, title in questions:
            num = qid.split("-", 1)[0]
            query_path = queries_dir / f"q{num}.{ext}"
            expected = args.root / "audit_questions" / qid / "expected_answer.json"
            if not query_path.exists() or not corpus_dir.exists():
                per_query.append(
                    QueryResult(
                        qid=qid,
                        title=title,
                        stack=stack,
                        status="bounty-open",
                    )
                )
                st.queries_open += 1
                continue
            r = run_query(
                query_path=query_path,
                corpus_dir=corpus_dir,
                expected_answer=expected,
                qid=qid,
                stack=stack,
                title=title,
            )
            per_query.append(r)
            st.queries_run += 1
            if r.status == "ok":
                st.queries_correct += 1
                st.total_loc += r.loc or 0
                st.total_wall_seconds += r.wall_seconds or 0
                print(
                    f"[{stack}] {qid}: {r.loc} LOC, {r.wall_seconds:.3f}s",
                    file=sys.stderr,
                )
            else:
                print(
                    f"[{stack}] {qid}: {r.status} ({r.error})",
                    file=sys.stderr,
                )
        if st.queries_correct > 0:
            print(
                f"[{stack}] total: {st.total_loc} LOC, "
                f"{st.total_wall_seconds:.3f}s wall",
                file=sys.stderr,
            )
        totals.append(st)

    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(
        render_results_md(questions, per_query, totals), encoding="utf-8"
    )

    wrong = [r for r in per_query if r.status == "wrong-answer"]
    if wrong:
        for r in wrong:
            print(
                f"WRONG: {r.stack}/{r.qid} — {r.error}",
                file=sys.stderr,
            )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
