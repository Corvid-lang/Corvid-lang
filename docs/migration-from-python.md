# Migration From Python

Corvid replaces library-level AI workflow conventions with compiler-checked language constructs.

## Mapping

- Python function calling wrappers -> `tool`
- prompt templates in strings -> `prompt`
- orchestration functions -> `agent`
- runtime approval middleware -> `approve`
- tracing/replay SDKs -> `corvid trace`, `corvid replay`
- policy linting scripts -> `corvid audit`

## First migration pass

1. Move irreversible external actions into `tool` declarations.
2. Mark approval boundaries explicitly with `approve`.
3. Put model-backed generation into `prompt`.
4. Add `Grounded<T>` where retrieval provenance is part of correctness.
5. Run `corvid doctor`, `corvid audit`, and your trace/eval suite.
