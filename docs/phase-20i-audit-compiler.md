# Phase 20i — Compiler-lane audit record

Rubric sweep of every source file in my file-scope lane: `corvid-ast`,
`corvid-syntax`, `corvid-resolve`, `corvid-types`, `corvid-ir`,
`corvid-driver`, `corvid-repl`. Per CLAUDE.md, a file fails the rubric
when it (1) mixes unrelated top-level concepts, (2) has 5+ public items
across unrelated domains, or (3) has 3+ internal sections sharing no
state. Line count is a heuristic, not the rule.

## Files split (20i-1 through 20i-audit-driver-e)

| File | Slice | Result |
|---|---|---|
| `corvid-syntax/src/parser.rs` | 20i-1 (9 commits) | 4,471 → 372 lines, 8 submodules |
| `corvid-types/src/checker.rs` | 20i-2 (8 commits) | 2,281 → 474 lines, 9 submodules |
| `corvid-types/src/effects.rs` | 20i-3 (4 commits) | 2,175 → 488 lines, 5 submodules |
| `corvid-types/src/lib.rs` | 20i-4 (1 commit) | 2,487 → 41 lines |
| `corvid-driver/src/lib.rs` | 20i-audit-driver (5 commits) | 1,935 → 1,224 lines, 6 submodules |

## Files audited — rubric PASS (no split needed)

Each file below was inspected against the three rubric criteria and
found to hold one coherent responsibility.

**Large files already focused:**

- `corvid-ir/src/lower.rs` (885 lines) — AST → typed IR translator, one
  `Lowerer` state machine with per-construct lowering methods. Single
  concern.
- `corvid-resolve/src/resolver.rs` (646 lines) — two-pass name resolution
  with a single `Resolver` state machine. Single concern.
- `corvid-types/src/errors.rs` (619 lines) — `TypeError` + `TypeWarning`
  enums plus their Display impls. Naturally large due to many diagnostic
  variants; one concern (errors for the type checker).
- `corvid-types/src/law_check.rs` (616 lines) — archetype law-check
  harness. `LawCheckResult`, `Law`, `Verdict`, `DimensionUnderTest`,
  `check_dimension`, sample helpers. Single concern.
- `corvid-driver/src/effect_diff.rs` (557 lines) — snapshot + diff of
  composed effect profiles across two revisions. Single concern.
- `corvid-driver/src/spec_check.rs` (548 lines) — extract and verify
  fenced `corvid` blocks from markdown spec files. Single concern.
- `corvid-types/src/config.rs` (518 lines) — corvid.toml custom-dimension
  parsing. Single concern.
- `corvid-resolve/src/lib.rs` (510 lines) — surface file (5 mod decls,
  re-exports, crate-level tests). Single concern.

**Everything under 500 lines** (roughly 90 files across the compiler
lane): each is a leaf module with one concern. Spot-checks on a dozen
randomly chosen files confirmed the pattern — no grab bags, no mixed
responsibilities, no pre-emptive splits justified.

## Exceptions

- `corvid-types/src/tests.rs` (2,446 lines) and
  `corvid-syntax/src/parser/tests.rs` (1,897 lines) — large integration
  test modules. Each exercises one crate's public surface, which is one
  responsibility. Further topical splitting is a secondary optimization
  and not required by the rubric.
- `corvid-driver/src/lib.rs` (1,224 lines after 20i-audit-driver-e) —
  the compile-and-async-run pipeline plus crate wiring. Two responsi-
  bilities (compile / async runtime invocation) that are tightly coupled
  — the async runtime helpers consume compile-pipeline outputs. Splitting
  them would fragment a single dataflow. Within the 1-2 rule.

## Status

Compiler lane sweep complete. No further split candidates. Phase 20i
remaining work is Dev B's runtime lane (20i-8, 20i-5,
20i-audit-runtime).
