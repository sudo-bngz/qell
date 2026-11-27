# Languages and language servers

## Highlighting

Tree-sitter grammars highlight Rust, C, Go, JavaScript, TypeScript/TSX,
HCL/Terraform, VCL (Varnish), Markdown, HTML, CSS, JSON, TOML, YAML, XML,
INI, shell scripts (including `.env` and dotfiles like `.bashrc`),
Makefiles, Dockerfiles and nginx configuration. Other `.conf`-style files
(Apache, sshd, MySQL, systemd…) get a tolerant generic highlighter.
[alea](https://github.com/sudo-bngz/alea) works (`.alea`) have their own.

Embedded code is highlighted too: fenced blocks in Markdown,
`<script>` and `<style>` in HTML, `RUN` lines in Dockerfiles, recipes in
Makefiles.

## Language servers

Qell starts the language server for a file when you open it, rooted at the
**project** (the nearest folder with `Cargo.toml`, `go.mod`,
`package.json`, `.git`…), and shares it between every file of that project.
Opening a folder that holds many projects starts one server per project
you actually open files in.

| Language | Server Qell looks for |
|---|---|
| Rust | `rust-analyzer` |
| Go | `gopls` |
| TypeScript, JavaScript | `typescript-language-server --stdio` |
| C, C++ | `clangd` |
| Terraform | `terraform-ls serve` |

Servers are found on your `PATH` and **never installed by Qell**. When one
is missing the status bar says *LSP not installed*; hover it for the
command to run. To use another server, or one outside your `PATH`, set it
in `config.toml`:

```toml
[lsp]
rust = ["/opt/rust-analyzer/bin/rust-analyzer"]
terraform = ["tofu-ls", "serve"]
```

## What a server gives you

- **Completion** as you type, with an icon per kind, the typed letters
  highlighted, documentation beside the list, auto-imports and snippets
  you `Tab` through. `Ctrl+Space` opens it by hand. Without a server,
  completion offers words from the file.
- **Go to definition** — `F12`, or `Ctrl`/`⌘`+click. Holding the key
  underlines what you can click. **Find all references** — `Shift+F12`.
  `Alt+←` comes back.
- **Rename** — `F2` renames a symbol everywhere the server finds it, in
  open and closed files, as one undo step per file.
- **Quick fixes** — `Ctrl/⌘+.` lists what the server can do where the
  cursor is: add an import, fill a struct, fix a typo…
- **Signature help** — typing `(` or `,` in a call shows the function's
  signature above the cursor with the parameter you're filling in
  underlined. `Ctrl/⌘+Shift+Space` opens it by hand, `Esc` closes it.
- **Inlay hints** — types and parameter names shown dimmed inside the
  code (`const total: number[]`, `greet(name: "hi")`). They aren't part of
  the file: the cursor steps over them. Qell turns them on for the
  TypeScript server and gopls, which keep them off by default. *Settings →
  Inlay Hints* hides them.
- **Go to symbol** — `Ctrl/⌘+Shift+O`. Qell reads symbols itself for the
  languages it parses, and asks the server for the others.

## Problems

Errors and warnings are underlined in the code, with the message at the
end of the line. The **PROBLEMS** tab in the panel lists them all, by file
(`Ctrl/⌘+Shift+M`); click one to go there.

Files with errors have a curly red line under their name in the file tree,
and so do the folders holding them. Right-click a file to list its errors,
run *Quick Fix…* or copy them; right-click a line in the editor to read its
message in full.

## Formatting

`Shift+Alt+F` formats the file; *Format on Save* (Settings) does it on
every save. Qell uses the language server when it can, or a command you
set — see [Configuration](configuration.md#formatters).
