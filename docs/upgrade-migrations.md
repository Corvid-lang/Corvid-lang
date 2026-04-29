# Corvid Upgrade And Migration Tools

`corvid upgrade` is the supported entrypoint for source, stdlib, schema, trace-format, and connector-manifest migrations. It is designed for CI first: `check` reports pending changes and exits non-zero when work is required; `apply` performs safe rewrites and prints the same report.

## Source And Stdlib

```bash
corvid upgrade check my_app/
corvid upgrade check my_app/ --json
corvid upgrade apply my_app/
```

The v1 source/stdlib migrator currently performs these safe rewrites:

- `syntax.pub_extern_agent_single_line`: migrates legacy split-line `pub extern "c"` plus `agent` into the stable `pub extern "c" agent` spelling.
- `stdlib.llm_complete_to_agent_run`: migrates `std.llm.complete(` calls to policy-aware `std.agent.run(` calls.
- `stdlib.cache_get_or_create_to_remember`: migrates `std.cache.get_or_create(` calls to `std.cache.remember(` calls.

Each finding includes a stable rule id, migration kind, file path, occurrence count, and replacement text. Rules that cannot be safely rewritten must be emitted as hand-review findings instead of silently changing source.

## CI Contract

For an application repository:

```bash
corvid upgrade check . --json > target/corvid-upgrade-report.json
```

Exit code `0` means no known migration is pending. Exit code `1` means at least one migration is required. A stable release is blocked if a public breaking change exists without a corresponding `corvid upgrade` rule or explicit non-scope note in the migration guide.

## Schema, Trace, And Connector Manifests

Schema, trace-format, and connector-manifest migrations use the same command family and report shape:

- `schema.migration_state_v1`: migrates migration-state JSON from `corvid.migration_state.v0` to `corvid.migration_state.v1`.
- `trace.format_v1`: migrates trace envelopes from `corvid.trace.v0` to `corvid.trace.v1`.
- `connector.manifest_v1`: migrates connector manifests from `manifest_version` `0.1` to `1.0`.

These rewrites are intentionally narrow and exact-match. If a future schema change needs structural conversion, the rule must parse JSON into typed data and emit hand-review findings for ambiguous cases.
