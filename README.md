# Qell

A minimalist code editor in Rust. Think VSCodium's look, without the
Electron: one native window, a file tree, highlighted code, a terminal at the
bottom, and language-server completion for the languages you actually use.

> Status: early. It opens, edits and saves files, highlights Rust, C, Go,
> JavaScript, TypeScript and Terraform, completes code through LSP, and runs
> your shell. Expect rough edges.

## Features

- **Editor** — rope-backed buffer, multi-tab, undo/redo, word movement,
  auto-indent, selection, line/column status.
- **Syntax highlighting** — tree-sitter grammars for Rust, C, Go,
  JavaScript, TypeScript/TSX and HCL/Terraform. VCL opens as plain text until
  a grammar exists.
- **Completion & diagnostics** — a JSON-RPC LSP client that launches
  `rust-analyzer`, `clangd`, `gopls`, `typescript-language-server` or
  `terraform-ls` on demand. Errors are underlined; hover a line to read them.
  Without a server, completion falls back to words from the buffer.
- **Integrated terminal** — the Alacritty terminal core in a panel
  (`Ctrl+\``), with 256/true colour, scrollback and application-cursor mode.
- **Plugins** — WebAssembly modules loaded from `~/.config/qell/plugins`,
  sandboxed by Wasmtime, exposing commands to the palette. See
  [`plugins/hello`](plugins/hello).
- **Command palette** (`Ctrl+Shift+P`), **quick open** (`Ctrl+P`),
  **go to line** (`Ctrl+G`).

## Install

Requires a Rust toolchain (1.95+). On Linux you also need the usual
windowing libraries (`libxkbcommon`, `libwayland`, `libgl`, and for the file
dialogs `libgtk-3` or `xdg-desktop-portal`).

```sh
cargo install --path crates/qell-app
qell                # current directory as workspace
qell path/to/dir    # or a folder
qell src/main.rs    # or a file
```

Language servers are found on `$PATH`. Install the ones you need, for example:

```sh
rustup component add rust-analyzer
go install golang.org/x/tools/gopls@latest
npm i -g typescript typescript-language-server
```

## Keys

| Action                | Shortcut           |
| --------------------- | ------------------ |
| Command palette       | `Ctrl+Shift+P`     |
| Go to file            | `Ctrl+P`           |
| Go to line            | `Ctrl+G`           |
| Save / Save as        | `Ctrl+S` / `Ctrl+Shift+S` |
| Open file / folder    | `Ctrl+O` / `Ctrl+Shift+O` |
| Close tab             | `Ctrl+W`           |
| Next / previous tab   | `Ctrl+Tab` / `Ctrl+Shift+Tab` |
| Undo / redo           | `Ctrl+Z` / `Ctrl+Y` |
| Trigger completion    | `Ctrl+Space`       |
| Toggle sidebar        | `Ctrl+B`           |
| Toggle terminal       | `` Ctrl+` ``       |
| Zoom                  | `Ctrl+=` / `Ctrl+-` |

On macOS, `Ctrl` is `⌘` for the editor shortcuts.

## Configuration

`~/.config/qell/config.toml` is written with defaults on first run:

```toml
font_size = 14.0
tab_size = 4
show_sidebar = true
show_terminal = true
auto_complete = true
ignore = [".git", "target", "node_modules", ".terraform", "dist"]

[lsp]
# Override a language server command:
# rust = ["rust-analyzer"]
```

## Architecture

```
crates/
  qell-core      text buffer (ropey), cursor, undo, tree-sitter highlighting, themes
  qell-lsp       LSP client: process + JSON-RPC framing + document sync + completion
  qell-term      PTY + VT emulation (alacritty_terminal) rendered as plain cell snapshots
  qell-plugins   Wasmtime host and the plugin ABI
  qell-app       the egui/eframe application: panels, keymap, palette
plugins/
  hello          example plugin (Rust → wasm32-unknown-unknown)
```

The core crates have no UI dependency, so the editor engine, the terminal
and the plugin host could be reused by a TUI or another toolkit.

### Adding a language

1. Add the tree-sitter grammar crate to `Cargo.toml`.
2. Add an entry to `builtin_languages()` in `crates/qell-core/src/syntax.rs`
   with its extensions, queries and language-server command.

That's it: highlighting, the LSP registry and the status bar pick it up.

## Development

```sh
cargo test --workspace        # unit tests (no display needed)
cargo run -p qell -- .        # run the editor on this repo
cargo clippy --workspace      # lints
```

## License

MIT or Apache-2.0, at your option.
