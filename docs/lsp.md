# Corvid LSP

Phase 24 starts with a reusable language-server core rather than an editor-only
prototype. The first shipped layer is transport-independent diagnostics:

- `DocumentSnapshot` holds an open document URI and text.
- `analyze_document` runs the real Corvid frontend through `corvid-driver`.
- Compiler diagnostics become standard `lsp_types::Diagnostic` values.
- Byte spans are converted to zero-based LSP ranges with UTF-16 columns.
- Compiler hints are preserved inside the diagnostic message.

This layer intentionally has no JSON-RPC server yet. The next slice adds the
stdin/stdout language-server transport and publishes diagnostics for
`textDocument/didOpen` and `textDocument/didChange`. Keeping analysis separate
prevents hover, completion, and navigation from growing their own compiler
pipelines.

Current validation:

```text
cargo test -p corvid-lsp
cargo check --workspace
```
