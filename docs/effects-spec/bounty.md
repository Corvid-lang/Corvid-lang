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
