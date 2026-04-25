# Corvid VS Code Extension

Reference VS Code client for `corvid-lsp`.

## Features

- Registers `.cor` files as Corvid.
- Starts `corvid-lsp` over stdio.
- Wires live diagnostics, hover, completion, go-to-definition, references,
  rename, and workspace symbols through the standard LSP client.
- Adds Corvid syntax highlighting, language configuration, and snippets for
  agents, prompts, dangerous tools, effects, and model catalog entries.

## Language Server Discovery

The extension resolves the server in this order:

1. `corvid.lsp.path` setting.
2. `CORVID_LSP_PATH` environment variable.
3. Repository-local `target/debug/corvid-lsp(.exe)`.
4. Repository-local `target/release/corvid-lsp(.exe)`.
5. `corvid-lsp` on `PATH`.

Build the server from the repository with:

```powershell
cargo build -p corvid-lsp
```

## Verify

```powershell
npm run verify
```

The verification script validates extension JSON files and checks the JavaScript
entrypoint syntax without requiring a packaged `.vsix`.
