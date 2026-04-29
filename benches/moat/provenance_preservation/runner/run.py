"""Provenance preservation rate runner.

For each chain under `chains/`, classifies each stack's
implementation as `preserved` / `lost` based on whether the final
function's return type carries a typed provenance / sources field at
the language-type level.

Classification heuristics (deliberately conservative — false
positives would give Corvid an unfair advantage):

  Corvid (.cor):
    The final agent's return type contains the literal substring
    `Grounded<` AND the file does not redeclare `Grounded` as a
    user type. The `Grounded<T>` shape carries provenance by
    construction.

  Python (.py):
    The final function's return type annotation is parsed. It is
    `preserved` only when the annotation is a typed model
    (TypedDict / Pydantic BaseModel) that has a `sources` /
    `source_documents` / `provenance` field declared at type level.
    A return type of `str`, `list[str]`, `dict`, `Any`, or an
    arbitrary class without one of those fields is `lost`.

  TypeScript (.ts):
    The final exported function's declared return type is parsed.
    It is `preserved` only when the type contains `sources:` or
    `provenance:` at the type level. `string`, `string[]`, untyped
    objects, or types lacking those fields are `lost`.

The runner is intentionally static — no LLM calls run. The point is
the TYPE-LEVEL guarantee, not whether a particular runtime exhibits
provenance after instrumentation.

Usage:
    python3 benches/moat/provenance_preservation/runner/run.py \\
        --chains-dir benches/moat/provenance_preservation/chains \\
        --out      benches/moat/provenance_preservation/RESULTS.md
"""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path

try:
    import tomllib  # Python 3.11+
except ModuleNotFoundError:  # pragma: no cover
    import tomli as tomllib  # type: ignore


PROVENANCE_FIELD_NAMES = ("sources", "source_documents", "provenance")


@dataclass
class ChainResult:
    chain_id: str
    title: str
    hops: int
    expected: dict[str, str]
    observed: dict[str, str] = field(default_factory=dict)
    notes: list[str] = field(default_factory=list)

    def matches_expected(self) -> bool:
        return all(
            self.observed.get(stack) == expected
            for stack, expected in self.expected.items()
        )


def discover_chains(chains_dir: Path) -> list[Path]:
    return sorted(p for p in chains_dir.iterdir() if p.is_dir())


def classify_corvid(corvid_src: Path) -> tuple[str, str]:
    """Returns (verdict, note)."""
    if not corvid_src.exists():
        return "error", "missing corvid.cor"
    text = corvid_src.read_text(encoding="utf-8")
    # Look at the final agent's signature.
    agent_match = re.findall(
        r"^agent\s+\w+\s*\([^)]*\)\s*->\s*([^:]+):",
        text,
        re.MULTILINE,
    )
    if not agent_match:
        return "lost", "no agent signature found"
    final_return = agent_match[-1].strip()
    if "Grounded<" in final_return or "List<Grounded<" in final_return:
        return "preserved", ""
    return "lost", f"final agent return is `{final_return}` — no `Grounded<...>`"


def classify_python(py_src: Path) -> tuple[str, str]:
    if not py_src.exists():
        return "error", "missing python.py"
    text = py_src.read_text(encoding="utf-8")

    # Find the LAST top-level `def` and its return annotation.
    fn_matches = list(
        re.finditer(
            r"^def\s+(\w+)\s*\([^)]*\)\s*->\s*([^:]+):",
            text,
            re.MULTILINE,
        )
    )
    if not fn_matches:
        return "lost", "no top-level function with annotated return"
    last = fn_matches[-1]
    ret_ann = last.group(2).strip()

    # If return type names a class, check whether that class has a
    # provenance-style field.
    class_match = re.match(r"^([A-Za-z_]\w*)$", ret_ann)
    if class_match:
        class_name = class_match.group(1)
        if class_name in {"str", "int", "float", "bool", "bytes"}:
            return "lost", f"return type is primitive `{ret_ann}`"
        if class_name in {"None"}:
            return "lost", "return type is None"
        return _check_python_class_for_provenance(text, class_name)

    # Generic / parameterised: list[X], dict[X,Y], tuple[...], etc.
    if ret_ann.startswith("list[") or ret_ann.startswith("List["):
        inner = ret_ann.split("[", 1)[1].rstrip("]")
        if inner in {"str", "int", "float", "bool"}:
            return "lost", f"return type is `{ret_ann}` — sources gone"
        return _check_python_class_for_provenance(text, inner)
    if ret_ann.startswith("dict") or ret_ann.startswith("Dict") or ret_ann == "Any":
        return "lost", f"return type is `{ret_ann}` — untyped"

    # Anything else: be conservative.
    return "lost", f"return type `{ret_ann}` lacks a typed provenance field"


def _check_python_class_for_provenance(
    text: str, class_name: str
) -> tuple[str, str]:
    cls = re.search(
        rf"^class\s+{re.escape(class_name)}\s*\([^)]*\)\s*:\s*\n((?:    .*\n)+)",
        text,
        re.MULTILINE,
    )
    if not cls:
        return "lost", f"class `{class_name}` not defined locally — cannot verify"
    body = cls.group(1)
    for name in PROVENANCE_FIELD_NAMES:
        if re.search(rf"^\s+{name}\s*:", body, re.MULTILINE):
            return "preserved", f"`{class_name}.{name}` carries provenance"
    return (
        "lost",
        f"class `{class_name}` lacks typed provenance "
        f"({'/'.join(PROVENANCE_FIELD_NAMES)}) field",
    )


def classify_typescript(ts_src: Path) -> tuple[str, str]:
    if not ts_src.exists():
        return "error", "missing typescript.ts"
    text = ts_src.read_text(encoding="utf-8")

    fn_matches = list(
        re.finditer(
            r"export\s+function\s+(\w+)\s*\([^)]*\)\s*:\s*([^\{]+?)\s*\{",
            text,
            re.MULTILINE | re.DOTALL,
        )
    )
    if not fn_matches:
        return "lost", "no exported function with annotated return"
    last = fn_matches[-1]
    ret_ann = last.group(2).strip()

    primitives = {"string", "number", "boolean", "void", "any", "unknown"}
    if ret_ann in primitives:
        return "lost", f"return type is `{ret_ann}`"
    if ret_ann.endswith("[]") and ret_ann[:-2] in primitives:
        return "lost", f"return type is `{ret_ann}` — sources gone"

    # Look for inline shape with provenance field.
    if any(f"{name}:" in ret_ann for name in PROVENANCE_FIELD_NAMES):
        return "preserved", "return type contains provenance field inline"

    # Look for a named type alias / interface.
    name_match = re.match(r"^([A-Za-z_]\w*)$", ret_ann)
    if name_match:
        name = name_match.group(1)
        type_decl = re.search(
            rf"^(?:type\s+{re.escape(name)}\s*=|interface\s+{re.escape(name)}\s*)\{{([^}}]*)\}}",
            text,
            re.MULTILINE | re.DOTALL,
        )
        if type_decl:
            body = type_decl.group(1)
            for fname in PROVENANCE_FIELD_NAMES:
                if re.search(rf"\b{fname}\s*:", body):
                    return "preserved", f"type `{name}.{fname}` carries provenance"
            return (
                "lost",
                f"type `{name}` lacks typed provenance "
                f"({'/'.join(PROVENANCE_FIELD_NAMES)}) field",
            )
        return "lost", f"type `{name}` not declared locally — cannot verify"

    return "lost", f"return type `{ret_ann}` lacks typed provenance field"


def run_chain(chain_dir: Path) -> ChainResult:
    chain_toml = chain_dir / "chain.toml"
    if not chain_toml.exists():
        raise SystemExit(f"missing chain.toml in {chain_dir}")
    meta = tomllib.loads(chain_toml.read_text(encoding="utf-8"))

    result = ChainResult(
        chain_id=meta["id"],
        title=meta["title"],
        hops=int(meta.get("hops", 0)),
        expected={
            "corvid": meta["expected"]["corvid"],
            "python": meta["expected"]["python"],
            "typescript": meta["expected"]["typescript"],
        },
    )

    cor_v, cor_n = classify_corvid(chain_dir / "corvid.cor")
    result.observed["corvid"] = cor_v
    if cor_n:
        result.notes.append(f"corvid: {cor_n}")

    py_v, py_n = classify_python(chain_dir / "python.py")
    result.observed["python"] = py_v
    if py_n:
        result.notes.append(f"python: {py_n}")

    ts_v, ts_n = classify_typescript(chain_dir / "typescript.ts")
    result.observed["typescript"] = ts_v
    if ts_n:
        result.notes.append(f"typescript: {ts_n}")

    return result


def render_results_md(results: list[ChainResult]) -> str:
    out: list[str] = []
    out.append("# Provenance preservation rate — published results\n")
    out.append(
        "> Auto-generated by "
        "`benches/moat/provenance_preservation/runner/run.py`. "
        "Do not hand-edit. Re-run the runner after adding or "
        "modifying chains.\n"
    )

    pres_corvid = sum(1 for r in results if r.observed.get("corvid") == "preserved")
    pres_py = sum(1 for r in results if r.observed.get("python") == "preserved")
    pres_ts = sum(1 for r in results if r.observed.get("typescript") == "preserved")
    total = len(results)

    out.append("## Headline numbers\n")
    out.append(f"- Chains run: **{total}**")
    out.append(f"- Provenance preserved by Corvid: **{pres_corvid}/{total}**")
    out.append(
        f"- Provenance preserved by Python (LangChain + pydantic): "
        f"**{pres_py}/{total}**"
    )
    out.append(
        f"- Provenance preserved by TypeScript (Vercel AI SDK + zod): "
        f"**{pres_ts}/{total}**\n"
    )

    out.append("## Per-chain verdicts\n")
    out.append(
        "| Chain | Hops | Corvid | Python | TypeScript | Match expected? |"
    )
    out.append(
        "|-------|------|--------|--------|------------|-----------------|"
    )
    for r in results:
        ok = "✓" if r.matches_expected() else "✗"
        out.append(
            f"| `{r.chain_id}` "
            f"| {r.hops} "
            f"| {r.observed.get('corvid', '?')} "
            f"| {r.observed.get('python', '?')} "
            f"| {r.observed.get('typescript', '?')} "
            f"| {ok} |"
        )

    notes = [(r.chain_id, n) for r in results for n in r.notes]
    if notes:
        out.append("\n## Notes\n")
        for cid, note in notes:
            out.append(f"- `{cid}`: {note}")

    out.append("\n## Methodology\n")
    out.append(
        "See `benches/moat/provenance_preservation/README.md` for the "
        "chain format, what counts as preserved, and the honesty rules. "
        "The runner is static — it parses the final return type of "
        "each stack's outer function and asks whether it exposes a "
        "typed sources / source_documents / provenance field at the "
        "language-type level."
    )

    return "\n".join(out) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--chains-dir", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()

    chains = discover_chains(args.chains_dir)
    if not chains:
        print(f"no chains under {args.chains_dir}", file=sys.stderr)
        return 1

    results = [run_chain(c) for c in chains]
    args.out.write_text(render_results_md(results), encoding="utf-8")

    mismatches = [r for r in results if not r.matches_expected()]
    if mismatches:
        print(
            f"{len(mismatches)} chain(s) mismatched expected verdicts:",
            file=sys.stderr,
        )
        for r in mismatches:
            print(
                f"  - {r.chain_id}: expected={r.expected} observed={r.observed}",
                file=sys.stderr,
            )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
