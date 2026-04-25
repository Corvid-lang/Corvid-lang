# Project guidance

Rules that apply to every piece of work in this repo. Machine-enforceable where possible; otherwise honored by convention.

## File responsibility discipline

**Every source file holds 1–2 responsibilities.** Line count is a *heuristic for where to look* — it is not the rule.

A file fails this discipline when any of these hold:

1. It mixes unrelated top-level concepts (parsing + lexing; checking + IR lowering; dispatch + recording).
2. It has 5+ public items representing unrelated domains.
3. It has 3+ internal sections that share no state.

A 3,000-line file that does one thing cleanly is fine. A 400-line grab bag is not.

**When a file fails the rubric:**

- If you are already modifying the file for a feature, split it in the same branch but as a separate commit that precedes the feature commit.
- If the file is untouched by your current work, file it as a Phase 20i/j audit candidate and move on — do not refactor pre-emptively.

**When splitting:**

- One commit per file extraction. No batching.
- Validation gate between every commit: `cargo check --workspace` + targeted `cargo test -p <crate> --lib` + `cargo run -q -p corvid-cli -- verify --corpus tests/corpus` (must still exit `1` only on the two deliberate fixtures).
- Push before starting the next extraction.
- Wait for acknowledgement at slice boundaries in parallel work — no autonomous chaining.
- Zero semantic changes during a refactor commit. Move code, add `pub use` re-exports to preserve the public API, nothing else. Bugs spotted mid-refactor get a separate branch.
- Commit message: `refactor(<crate>): extract <responsibility> from <file>` — body names which rubric criterion failed and how the split resolves it.

## No shortcuts

The hardest rule. No shortcuts anywhere. If a shipped surface conflicts with the spec, fix the surface rather than softening the spec (see `memory/project_phase_20_closed.md` for the canonical example).

## Invention shipping contract

Every new invention ships with public proof at the same time as the code. A feature counts as an invention when it is a Corvid-specific language/runtime capability that we would name in the README, site, launch material, or HN discussion.

Before an invention slice is complete, it must include:

- A README catalog entry or an explicit update explaining why the existing entry already covers it.
- A `corvid tour --topic <name>` demo whose source compiles through the normal driver pipeline.
- A `docs/inventions.md` proof-matrix row with shipped status, runnable command, test coverage, spec link, and explicit non-scope.
- A spec or reference-doc link that defines the behavior.
- Tests that validate the behavior named in the catalog entry.

Do not ship hidden inventions. If the feature is important enough to make Corvid extraordinary, it is important enough to be discoverable, runnable, and test-backed.

## Pre-phase chat mandatory

Never start code on a phase or slice until we've chatted and both sides understand the scope. This applies to refactor slices too — a decomposition plan gets agreed before any file moves.

## Commit at slice boundaries

A slice is not done until its commit is on `main`. Dev-log + git history + ROADMAP checklist must all agree before the next slice starts.

## Professional names in source

Code, doc comments, and docstrings use behavioral names for features. Slice numbers (`20h slice G`, `slice 17f`) appear only in commit messages, `ROADMAP.md`, `dev-log.md`, `learnings.md`, and phase-specific doc files.

## Track deferred work in ROADMAP

In-progress phases carry slice-by-slice `- [ ]` checklists so deferred work never falls through the cracks.

## Update `learnings.md` per user-visible slice

Doc-and-feature land together. No drift between shipped behavior and the learnings record.

## Follow ROADMAP order strictly

Work proceeds in the defined sequence. Never skip ahead or reorder without an explicit pre-phase chat that amends the ROADMAP first.
