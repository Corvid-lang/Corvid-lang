# Corvid Effect-System Bounty

Corvid's core safety promise is compile-time: programs that violate approval,
budget, trust, reversibility, confidence, or groundedness constraints should not
compile. This page defines the public process for reporting bypasses and turning
accepted reports into permanent regression fixtures.

## What Counts

Submit a minimal `.cor` program if it does one of these:

- Performs or permits a `dangerous` operation without the compiler requiring the correct `approve`.
- Lets an agent violate `@budget`, `@trust`, `@reversible`, or `@min_confidence`.
- Returns `Grounded<T>` without a real `data: grounded` provenance path.
- Changes effect behavior across equivalent rewrites or execution tiers.
- Makes `corvid test spec --meta`, `corvid test adversarial`, or `corvid verify --corpus` miss a violation they should catch.

False positives are also useful: if a safe program is rejected, file it, but label
it as a false positive rather than a bypass.

## Submission Format

Use the GitHub issue template **Effect-system bypass report**. Include:

- The complete `.cor` program.
- The command you ran, usually `cargo run -q -p corvid-cli -- check file.cor`.
- The actual result and the expected result.
- Why the program should be legal or illegal under the spec.
- Environment details if the behavior depends on OS, target tier, or config.

Do not paste secrets, provider keys, private prompts, customer data, or real
production traces. Reduce the report to a minimal synthetic reproduction first.

## Triage

Reports are triaged into one of four outcomes:

| Outcome | Meaning | Action |
|---|---|---|
| Accepted bypass | The compiler accepted a program the spec says must reject. | Fix the checker, add the program to `docs/effects-spec/counterexamples/`, and credit the reporter. |
| Accepted false positive | The compiler rejected a valid program. | Fix the checker or clarify the spec, then add a positive regression test. |
| Spec ambiguity | The program exposes an unclear rule. | Clarify the spec before changing the compiler. |
| Not a bug | The program behaves as specified. | Close with the relevant spec link. |

## Disclosure

Corvid is pre-v1.0, but safety bypasses still deserve careful handling. If a
report includes a production exploit path or sensitive deployment details, open
a private security advisory instead of a public issue. Otherwise, public issues
are preferred because the counterexample corpus is meant to be auditable.

## Credit

Accepted bypasses keep reporter credit in the fixture header. The seed corpus
uses Corvid core-team credit only because those examples predate the public
process. Future accepted community reports should use this shape:

```corvid
# bypass: short_name
# reporter: @github-handle
# fixed_by: commit <sha>
# invariant: cost must compose by Sum
```

## Permanent Regression

Every accepted bypass must land with:

- A `.cor` counterexample fixture under `docs/effects-spec/counterexamples/`.
- A spec or dev-log note explaining the invariant.
- A test path that fails before the fix and passes after it.
- If relevant, an entry in `corvid test adversarial`'s deterministic taxonomy or seed corpus.

The bounty process is not marketing. It is how the safety claim gets stronger
over time.

## Open bounty window — moat benchmark corpora (opened 2026-04-29)

Two published moat-benchmark corpora are now under explicit bounty review.
Submissions that reduce Corvid's published advantage on either corpus will
be merged and credited.

### `benches/moat/compile_time_rejection/` — 50 named bug classes

Current published numbers:

- Corvid (`cargo run -q -p corvid-cli -- check`): **50/50 rejected**.
- Python (`mypy --strict + pydantic`): **0/50 rejected**.
- TypeScript (`tsc --strict + zod`): **0/50 rejected**.

A submission counts if it does any of:

1. Provides an idiomatic Python rewrite that makes `mypy --strict + pydantic`
   reject any of the 50 cases (including with additional pydantic validators
   a senior dev would actually use). The runner re-classifies that case from
   `accepted` to `rejected` for the Python column.
2. Provides an idiomatic TypeScript rewrite that makes `tsc --strict + zod`
   reject any case. Same effect on the TypeScript column.
3. Provides a Corvid program of the same shape that the typechecker
   incorrectly accepts. That promotes the case to a real Corvid bypass under
   the existing rules above and triggers a fix.

Honesty requirements for accepted submissions:

- Implementations must use libraries / patterns a senior dev would *actually*
  reach for. Custom AST-walking semgrep rules, hand-rolled clang-tidy-style
  linters, or one-off mypy plugins are accepted only if they ship as part of a
  documented project template — not as one-shot reviewer-only tools.
- The submission lands in `cases/<NN>-<slug>/` with the rewrite plus a note in
  `case.toml` explaining what the alternative baseline is.

### `benches/moat/provenance_preservation/` — multi-hop AI workflow chains

Current published numbers (3 of target 10 chains):

- Corvid: **3/3 preserved**.
- Python (LangChain + pydantic): **0/3 preserved**.
- TypeScript (Vercel AI SDK + zod): **0/3 preserved**.

A submission counts if it provides an idiomatic Python or TypeScript
implementation whose final return type *at the language-type level* exposes
a typed `sources` / `source_documents` / `provenance` field that traces
back to the original retrieval. The runner re-classifies the chain
accordingly and the headline number drops.

The static-only classification means: dict-shaped return values, manually
threaded metadata, or convention-only `# sources kept here` markers do not
count. The point is whether a downstream consumer can extract provenance
*without reading the implementation*.

### `benches/moat/replay_determinism/` — deterministic re-execution of `refund_bot_demo`

Current published numbers:

- Corvid (`cargo run -p refund_bot_demo`, N=20, normalized): **all pairs byte-identical**.
- Python (LangChain + LangSmith): **bounty-open** — no `_summary.json` yet.
- TypeScript (Vercel AI SDK + OTEL): **bounty-open** — no `_summary.json` yet.

The harness invokes the same agent N times against mocked tools and a
mocked LLM, normalizes wall-clock and run-id fields per the documented
rules, and asks whether all N traces are byte-identical pairwise.

A submission counts if it provides an idiomatic Python or TypeScript
implementation of the same agent (contract in
`benches/moat/replay_determinism/runs/corvid/agent_spec.md`) plus a
runner that loops N times, captures the stack-native trace artifact,
applies its documented normalization, and emits `_summary.json` in
the standard shape. The orchestrator re-renders `RESULTS.md` and
the headline updates accordingly.

Submissions are rejected if the normalization rules paper over real
divergence (e.g. stripping retry counters or attempt IDs that
genuinely vary across runs). The point is what the stack ships out
of the box — not how aggressively a reviewer can normalize.

### Window

The bounty window is rolling. The published numbers are
"as of the most recent CI run that ran the runner against the committed
corpus + bounty submissions." There is no fixed close date — the corpora
stay open and the published numbers stay current.

A submission that lands and reduces Corvid's advantage gets a credit row in
the relevant `RESULTS.md` and the launch wording adapts.
