# Benchmark — compile-time rejection rate

For each bug class in `cases/`, the harness asks three stacks to compile
the same intended behaviour. The result table records, per case, whether
each stack rejected the program **at compile / typecheck time** (before
any code ran).

## What counts as a rejection

| Stack | Counts as a rejection if... |
|---|---|
| **Corvid** | `cargo run -q -p corvid-cli -- check <case>/corvid.cor` exits non-zero AND the diagnostic carries the expected `guarantee_id`. |
| **Python + pydantic + mypy** | `mypy --strict <case>/python.py` exits non-zero, OR `python -c "from python import ..."` raises a `pydantic.ValidationError` at import-time (before any agent function runs). |
| **TypeScript (strict + zod)** | `tsc --strict --noEmit <case>/typescript.ts` exits non-zero, OR a `zod` schema parsed at module top-level rejects the offending shape. |

A *runtime* error caught only when the function is invoked does NOT count
as a rejection. The whole point is "would this program ship into prod and
fail there?"

## Case format

Each case lives under `cases/<NN>-<slug>/` and contains exactly four files:

- `case.toml` — metadata (see schema below).
- `corvid.cor` — the Corvid implementation. Must intentionally trigger
  the bug class.
- `python.py` — the equivalent Python program with `mypy --strict` +
  pydantic annotations a normal developer would write.
- `typescript.ts` — the equivalent TypeScript program with
  `--strict --noEmit` + zod schemas a normal developer would write.

### `case.toml` schema

```toml
# Stable case id. Match the directory name.
id = "01-unapproved-dangerous-call"

# Short title for the published table.
title = "Dangerous tool called without approval"

# Bug class — the canonical name in launch material.
bug_class = "approval-bypass"

# 1-paragraph description of the bug class.
description = """
The program calls a tool annotated as dangerous (financial impact,
external write, irreversible) without first obtaining human approval.
"""

# Expected rejection per stack. Each entry must be one of:
#   "rejected"    — stack catches at compile / typecheck time.
#   "accepted"    — stack compiles cleanly; bug ships into prod.
#   "runtime"     — stack accepts at compile time but a runtime check
#                    catches it before the dangerous side-effect (still
#                    weaker than compile-time rejection; counts as
#                    "accepted" in the published rejection rate).
[expected]
corvid = "rejected"
python = "accepted"
typescript = "accepted"

# When `corvid = "rejected"`, this is the registered guarantee_id the
# diagnostic must carry (verified against corvid_guarantees::GUARANTEE_REGISTRY
# by the runner). Empty when `corvid = "accepted"`.
corvid_guarantee_id = "approval.dangerous_call_requires_token"

# When the Python or TS stack uses a particular library/feature to
# attempt the rejection, name it here. Honest baseline.
python_baseline = "mypy --strict + pydantic"
typescript_baseline = "tsc --strict + zod"
```

## How to add a new case

1. Create `cases/<NN>-<slug>/`.
2. Write `case.toml` per the schema above. Pick the next unused `NN`.
3. Write `corvid.cor`, `python.py`, `typescript.ts`. Each must implement
   the same intended behaviour; the bug class must be present in all
   three.
4. Run the harness (see "Running" below). The runner auto-discovers the
   new case directory.
5. Commit. CI re-runs the harness and refuses the merge if `RESULTS.md`
   drifts from what the harness produces.

## Running

The runner is `runner/run.py`. From the repo root:

```bash
python3 benches/moat/compile_time_rejection/runner/run.py \
    --cases-dir benches/moat/compile_time_rejection/cases \
    --out benches/moat/compile_time_rejection/RESULTS.md
```

The runner shells out to:

- `cargo run -q -p corvid-cli -- check <corvid.cor>` for Corvid.
- `mypy --strict <python.py>` for Python.
- `tsc --strict --noEmit <typescript.ts>` for TypeScript.

Exit code 0 = stack accepted. Non-zero = stack rejected. The runner
also greps Corvid's diagnostic for the expected `guarantee_id` and
fails the case if the rejection happened *for the wrong reason*.

## Honesty rules

1. **No "winning by ignoring."** If a Python tool exists that rejects
   the bug class (even if it isn't normally part of a Python developer's
   pipeline), the runner uses it. The published table cites the exact
   tool used.
2. **No fake bug classes.** Every case must reflect a real shape that
   ships in production AI apps. Synthetic "Corvid wins, others lose"
   cases get rejected at code review.
3. **Equal idiomatic depth.** The Python / TS implementations must be
   what a senior developer in that ecosystem would actually write. No
   intentionally-naive baselines.
4. **Adversarial review.** Before publishing the numbers, the table is
   posted to the bounty page (`docs/effects-spec/bounty.md`) for at
   least 7 days; any submitted case that *should* be rejected by
   Corvid but isn't promotes to a real follow-up slice.

## Published corpus

Examples from the 50-case corpus:

- `01-unapproved-dangerous-call` — call a `@dangerous` tool without
  `approve`. Corvid `rejected` (guarantee `approval.dangerous_call_requires_token`).
- `02-effect-row-under-report` — declare an effect row that omits an
  effect actually produced in the body. Corvid `rejected` (guarantee
  `effect_row.body_completeness`).
- `03-grounded-without-citation` — return `Grounded<T>` with no retrieval
  in the call chain. Corvid `rejected` (guarantee `grounded.provenance_required`).

The full 50-case corpus is committed. `RESULTS.md` is the source of truth for
the current rejection table and must be regenerated by the runner after any
case, runner, or dependency change.
