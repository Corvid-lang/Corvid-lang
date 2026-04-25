# Corvid LSP

Phase 24 starts with a reusable language-server core rather than an editor-only
prototype. The first shipped layer is transport-independent diagnostics:

- `DocumentSnapshot` holds an open document URI and text.
- `analyze_document` runs the real Corvid frontend through `corvid-driver`.
- Compiler diagnostics become standard `lsp_types::Diagnostic` values.
- Byte spans are converted to zero-based LSP ranges with UTF-16 columns.
- Compiler hints are preserved inside the diagnostic message.

The stdio server is now wired as the `corvid-lsp` binary. It supports:

- `initialize` with full-document text sync capability.
- `shutdown` / `exit`.
- `textDocument/didOpen`.
- `textDocument/didChange`.
- `textDocument/didSave`.
- `textDocument/hover`.
- `textDocument/completion`.
- `textDocument/publishDiagnostics` notifications backed by the same compiler
  diagnostic path as the CLI.

The implementation keeps protocol concerns separated: `server.rs` owns JSON-RPC
method handling and document state, while `transport.rs` owns LSP
`Content-Length` framing over stdin/stdout.

Hover support is compiler-backed, not regex-based. `hover.rs` parses, resolves,
and typechecks the current document, then returns Markdown summaries for:

- inferred expression types;
- agent signatures and constraints;
- tool signatures, effect rows, and dangerous approval boundaries;
- prompt signatures, effect rows, calibration/cache flags, strict citations,
  and model-routing mode;
- type and effect declarations.

Completion support is context-aware and parser-backed. `completion.rs` uses the
current partial document so suggestions still work while the user is typing, and
keeps AI-native contexts specific:

- declaration, statement, and AI-native keywords;
- declared agents, tools, prompts, types, effects, models, and evals;
- PascalCase approval labels derived from dangerous tools after `approve`;
- declared effect names after `uses`;
- model catalog names in prompt-routing and escalation contexts.

Current validation:

```text
cargo test -p corvid-lsp
cargo check --workspace
```
