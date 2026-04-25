# Adversarial Bypass Taxonomy

`corvid test adversarial` turns the effect-system spec into attack prompts and
then runs every generated program through the compiler. The deterministic seed
path ships first so CI has a stable no-network safety gate; provider-backed LLM
sampling can feed more programs into the same classifier later.

## Categories

| Category | Invariant | Bypass Angles |
|---|---|---|
| `approval` | Dangerous tools require an in-scope `approve` with the right label and arity. | Direct dangerous calls, wrong approval scope, wrong approval label or arity. |
| `trust` | `@trust(autonomous)` cannot call `human_required` or `supervisor_required` effects. | Hide high-trust effects behind helper agents or renamed effects. |
| `budget` | `@budget` checks worst-case composed cost before runtime. | Split work across tools or helpers so total cost appears smaller. |
| `provenance` | `Grounded<T>` returns require a retrieval provenance chain. | Fabricate `Grounded<T>` from non-retrieval tools or aliases. |
| `reversibility` | `@reversible` excludes any call chain containing irreversible effects. | Hide `reversible: false` under neutral tool names or wrapper agents. |
| `confidence` | `@min_confidence` composes by minimum, not mean. | Mix strong and weak sources and rely on averaging intuition. |

## Generator Contract

The prompt asks a model to return JSONL objects with:

```json
{"category":"approval","title":"direct dangerous call","source":"...complete .cor program..."}
```

Each source must be a complete program and must attempt exactly one bypass.
The classifier treats every generated program as an expected rejection. If the
compiler accepts one, the row is marked `ESCAPED`; that is either a compiler
safety bug or an invalid generator prompt that must be reclassified before
release.

## Issue Filing

Set both environment variables to file escaped bypasses directly:

```bash
CORVID_ADVERSARIAL_FILE_ISSUES=1
GITHUB_TOKEN=...
corvid test adversarial --count 100 --model opus
```

Without those variables the command remains offline and CI-safe. Escapes still
exit non-zero; they are not silently ignored.
