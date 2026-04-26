# Corvid Cookbook

## Approval-gated action

Use `approve` before dangerous tools and verify the result with:

```sh
corvid audit path/to/app.cor
```

## Grounded retrieval flow

Use `Grounded<T>` values for retrieval-backed answers and inspect provenance with:

```sh
corvid trace dag <trace-id>
```

## Provider migration

Retest recorded traffic against another model:

```sh
corvid eval --swap-model <model> --source app.cor target/trace
```

## Package contract review

Inspect imports and package metadata:

```sh
corvid import-summary app.cor
corvid package metadata app.cor --name @scope/name --version 1.0.0
```
