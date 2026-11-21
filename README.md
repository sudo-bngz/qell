# Qell

A minimalist code editor in Rust. Think VSCodium's look, without the
Electron: one native window, a file tree, highlighted code, a terminal at the
bottom, and language-server completion for the languages you actually use.

> Status: early. It opens, edits and saves files, highlights about twenty
> languages and config formats, completes code through LSP, shows git
> status and runs your shells. Expect rough edges.

## Features

- **Editor** — rope-backed buffer, multi-tab, undo/redo, word movement,
  auto-indent, selection, line/column status.
- **Syntax highlighting** — tree-sitter grammars for Rust, C, Go,
  JavaScript, TypeScript/TSX, HCL/Terraform, VCL (Varnish), Markdown, HTML,
  CSS, JSON, TOML, YAML, XML, INI, shell scripts (including `.env` and
  dotfiles like `.bashrc`), Makefiles, Dockerfiles and nginx config. Other
  `.conf`-style files (Apache, sshd, MySQL, systemd…) get a tolerant
  generic highlighter for comments, sections, keys, strings, numbers and
  variables. [alea](https://github.com/sudo-bngz/alea) works (`.alea`) have
  their own highlighter, which keeps colouring half-typed code. Embedded code
  is highlighted too: fenced code blocks in
  Markdown, `<script>`/`<style>` in HTML, `RUN` in Dockerfiles, recipe lines
  in Makefiles.
- **File icons** — brand icons and colours per file type in the tree and
  tabs (Terraform, Rust, Go, Docker, JSON…), from a small bundled subset of
  Nerd Fonts; nothing to install.
- **Completion & diagnostics** — a JSON-RPC LSP client that launches
  `rust-analyzer`, `clangd`, `gopls`, `typescript-language-server` or
  `terraform-ls` on demand. VS Code-style suggestions: icons per kind,
  typed letters highlighted, a documentation panel beside the list,
  auto-imports, snippets with placeholders you Tab through, and the
  server's own trigger characters (`.`, `::`, `->`…). The list filters
  locally as you type and only goes back to the server when it has to.
  Errors are underlined; hover a line to read them. Without a server,
  completion falls back to words from the buffer.
  Qell never downloads or installs anything: when a server is missing, the
  status bar says "LSP not installed"; hover it for the command to run
  (click to copy it).
- **Git status** — the file tree and tabs are coloured like VS Code
  (modified, added, untracked, deleted, conflict, ignored), with a badge per
  file, a dot on folders containing changes, and the branch with
  ahead/behind counts in the status bar. Refreshed in the background.
  Works when the folder you open holds several repositories (found up to
  3 levels deep by default, `git_scan_depth`), or repositories nested in
  another one: each file is coloured by its own repository.
- **Source Control** (`Ctrl+Shift+G`) — VS Code-style panel: per
  repository, staged changes and changes with stage / unstage buttons and
  a commit box (`Ctrl+Enter`). Commits run your hooks and signing in the
  background; a failing hook shows its message. The activity bar badge
  counts changed files. A **Graph** section draws the history of HEAD and
  its upstream (branches, merges, tags). Click a commit to list the files
  it changed, click a file to open its diff (read-only, inline, with
  syntax colours; `F7` / `Shift+F7` jump between changes). Right-click a
  commit to copy its hash or message.
- **Integrated terminals** — the Alacritty terminal core in a panel
  (`` Ctrl+` ``), with 256/true colour, scrollback and application-cursor mode.
  Run several shells as tabs: `` Ctrl+Shift+` `` opens a new one,
  `Ctrl+PageUp`/`Ctrl+PageDown` switch between them, and typing `exit`
  closes a tab. Select text with the mouse (double-click: word, triple-click:
  line) and copy it with `⌘C` / `Ctrl+Shift+C`; right-click for Copy, Paste,
  Select All and Clear Terminal.
- **Plugins** — WebAssembly modules loaded from `~/.config/qell/plugins`,
  sandboxed by Wasmtime, exposing commands to the palette. See
  [`plugins/hello`](plugins/hello).
- **Change marks in the gutter** — lines added (green), modified (blue) or
  deleted (red wedge) since git's version of the file, updated as you type.
- **Vim mode** (Settings → Editor, or "View: Toggle Vim Mode") — Normal,
  Insert and Visual modes in the editor, with a command line under the code;
  the file tree, tabs, terminal and minimap stay as they are.
  - Motions: `h j k l`, `w b e`, `0 ^ $`, `gg G`, `{ }`, `%`, `f t F T ; ,`,
    `Ctrl-D`/`Ctrl-U`, counts.
  - Operators `d c y > <` with motions, lines (`dd`, `3yy`) and text objects
    (`ciw`, `di(`, `ca"`); `x D C s S J r ~ p P u Ctrl-R .`
  - `v` / `V` selections, `/` `?` `n` `N` `*` search (regex, highlighted).
  - `:w`, `:q`, `:wq`, `:x`, `:wa`, `:qa`, `:e file`, `:42`, `:bn`/`:bp`,
    `:s/a/b/g` and `:%s/…` (also on a selection), `:d`, `:noh`,
    `:set wrap`, `:term`, `:Ex`; `gd` goes to the definition, `gt`/`gT`
    switch tabs.
- **Right-click menus** — in the file tree (New File/Folder, Rename and
  Delete inline, Reveal in Finder, Open in Integrated Terminal, Copy Path),
  the editor (Go to Definition, References, Cut/Copy/Paste…) and the tabs
  (Close Others, Close to the Right, Close All…). Delete moves to the Trash
  after asking.
- **File tree shortcuts** — above the tree sit the folders you work in most
  (the repositories, in a workspace holding several), then the files you
  open most, each with the folder it's in. Clicking a folder expands it in
  the tree; clicking a file opens it and reveals it. Pin the ones you always
  want there. "MOST USED" can be switched to "RECENT".
- **Minimap** — the whole file in miniature on the right of the editor, in
  its syntax colours, with the visible part shaded: click to jump, drag to
  scroll. *View → Minimap* or Settings to hide it.
- **Word wrap** (`Alt+Z`, `⌥Z` on macOS, or Settings) — long lines wrap at
  word boundaries to the window's width, continuation rows keep the line's
  indentation, and Up/Down, the mouse and selections follow the wrapped rows.
- **Bracket pairs and indent guides** — brackets are coloured by nesting
  depth (unmatched ones in red, brackets in strings and comments ignored),
  the pair at the cursor is boxed, and vertical guides mark each indentation
  level, with the block around the cursor highlighted. The indentation step
  (2, 4, tabs…) is detected per file. Both can be turned off in Settings.
- **Stage or revert one change** — click a change mark in the gutter for
  *Revert* (put those lines back to git's version, in the editor and
  unsaved) or *Stage* (that change alone into the index, the rest of the
  file left as it is). Also *Edit → Git: Stage This Change* and the palette.
- **Replace across files** — the Search view's chevron opens a replace box;
  *Replace All* asks once, then changes every match. Files open in a tab are
  changed in the editor and left unsaved so you can read them over and undo;
  the others are written to disk.
- **Go to symbol in the file** (`Ctrl/⌘+Shift+O`) — everything the open file
  defines, in one list you can type into: Terraform blocks as `local.env` or
  `aws_s3_bucket.logs`, functions, types and methods elsewhere. Qell reads
  the file itself for the languages it knows and asks the language server
  for the rest.
- **Rename a symbol** (`F2`) through the language server, across every file
  it is used in. The files it touches open as tabs with unsaved changes, so
  nothing is written until you save them.
- **Quick fixes** (`Ctrl/⌘+.`) — what the language server can do at the
  cursor (remove an unused import, add a missing one, a refactoring), in a
  list to pick from.
- **Brackets and quotes close themselves** as you type, a selection gets
  wrapped when you type one around it, and backspace between an empty pair
  takes both halves. Off with `auto_close` in the settings.
- **Toggle comment** (`Ctrl/⌘+/`) — comments the lines the cursors are on,
  or uncomments them when they all already are, lining the markers up with
  the least indented line.
- **Several cursors** — `Ctrl/⌘+D` selects the word under the cursor and
  then one more occurrence each time, `Ctrl/⌘+Shift+L` takes them all,
  `Alt+click` puts a cursor anywhere (click again to remove it), and
  `Ctrl+Alt+↑/↓` (`⌥⌘↑/↓` on macOS) adds one on the line above or below.
  Typing, deleting, Enter and Tab happen at every cursor, pasting as many
  lines as there are cursors gives each its own, and `Escape` goes back to
  one.
- **Format** (`Shift+Alt+F`, and on save) — the language server when it
  offers formatting, otherwise the language's own tool: `gofmt`,
  `terraform fmt`, `rustfmt`, `black`, `clang-format`, `shfmt`, `taplo`,
  `prettier`. Nothing is installed for you: a tool is used if it's on your
  PATH, and `[format]` in the config overrides any of them. Only the lines
  that change are touched, so the cursor stays put and one undo reverts it.
- **Files changed outside Qell** — a `git checkout`, a `terraform fmt` or
  another editor is noticed: an untouched file is reloaded quietly, and one
  with unsaved edits asks whether to reload or keep yours. Saving never
  silently overwrites a change made elsewhere.
- **Session restore** — the files you had open come back when you reopen
  the folder, with the cursor where you left it (Settings → Layout).
- **Find and replace in the file** (`Ctrl/⌘+F`, `Ctrl+H` / `⌥⌘F`) — a bar
  over the editor with case, whole-word and regex toggles, every match
  highlighted, `Enter` / `Shift+Enter` (or `F3` / `Shift+F3`) to step
  through them, and Replace / Replace All (`$1` groups in regex mode).
- **Search in files** (`Ctrl+Shift+F`) — the workspace is indexed in memory
  in the background (respecting `.gitignore`), so results appear as you
  type, even in large trees. Case, whole-word and regex toggles; click a
  match to open it.
- **Go to definition** — `Ctrl`/`⌘`+click a function, type, variable…
  (the symbol is underlined while the key is held), or press `F12`.
  Qell asks the language server when one is installed, and otherwise uses
  its own index (tree-sitter tags for Rust, Go, C, JavaScript, TypeScript;
  `var.`, `local.`, `module.`, `data.` and resource addresses for
  Terraform). Several matches are listed in the Search view.
  `Shift+F12` finds all references; `Alt+←` / `Alt+→` (`⌃-` / `⌃⇧-` on
  macOS) go back and forward.
- **Command palette** (`Ctrl+Shift+P`), **quick open** (`Ctrl+P`),
  **go to line** (`Ctrl+G`).
- **Kubernetes cluster view** (`Ctrl/⌘+Shift+K`) — a k9s-style table where
  the editor is: pods, deployments, services, nodes, events and anything
  else your cluster serves, in a monospace list you drive from the
  keyboard. `j`/`k` move, `Enter` goes in (a namespace's pods, a workload's
  or a service's pods, a node's pods, a pod's containers), `Esc` comes
  back, `/` filters, `:` switches resource by name or short name (`:po`,
  `:svc`, `:ns`, `:ctx`), `0` shows every namespace, `r` refreshes, `?`
  lists the keys. Rows are coloured by what the cluster says: a
  `CrashLoopBackOff` pod and a `NotReady` node in red, a rollout still
  catching up in yellow.

  It reads everything through your own `kubectl`, so it uses your
  kubeconfig, your contexts and your credential plugins (`aws eks
  get-token` and the like) — Qell holds no credentials and installs
  nothing. The list refreshes every two seconds **only while the panel has
  the keyboard**, and stops the moment you click elsewhere; the header says
  which it is doing. Nothing talks to a cluster in the background.

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
| Toggle comment        | `Ctrl+/`           |
| Replace in files      | `Ctrl+Shift+H`     |
| Go to symbol in file  | `Ctrl+Shift+O`     |
| Rename symbol         | `F2`               |
| Quick fix             | `Ctrl+.`           |
| Add cursor at next occurrence | `Ctrl+D`     |
| Select all occurrences | `Ctrl+Shift+L`     |
| Add cursor above / below | `Ctrl+Alt+↑` / `Ctrl+Alt+↓` |
| Find / Replace in file | `Ctrl+F` / `Ctrl+H` |
| Find next / previous  | `F3` / `Shift+F3`  |
| Search in files       | `Ctrl+Shift+F`     |
| Go to definition      | `F12` / `Ctrl+click` |
| Find all references   | `Shift+F12`        |
| Back / forward        | `Alt+←` / `Alt+→` (`⌃-` / `⌃⇧-` on macOS) |
| Save / Save as        | `Ctrl+S` / `Ctrl+Shift+S` |
| Open file / folder    | `Ctrl+O` / `Ctrl+Alt+O` |
| Close tab             | `Ctrl+W`           |
| Next / previous tab   | `Ctrl+Tab` / `Ctrl+Shift+Tab` |
| Undo / redo           | `Ctrl+Z` / `Ctrl+Y` |
| Trigger completion    | `Ctrl+Space`       |
| Toggle sidebar        | `Ctrl+B`           |
| Toggle word wrap      | `Alt+Z`            |
| Explorer / Source Control | `Ctrl+Shift+E` / `Ctrl+Shift+G` |
| Kubernetes cluster    | `Ctrl+Shift+K`     |
| Toggle terminal       | `` Ctrl+` ``       |
| New terminal          | `` Ctrl+Shift+` `` |
| Next / prev terminal  | `Ctrl+PageDown/Up` |
| Zoom                  | `Ctrl+=` / `Ctrl+-` |
| Settings              | `Ctrl+,`           |

On macOS, `Ctrl` is `⌘` for the editor shortcuts, and Qell uses the native
menu bar (Qell / File / Edit / View / Go / Window, with About, Settings… `⌘,`,
Hide and Quit in the app menu) instead of the in-window one.

## Configuration

Open **Settings** with `Ctrl+,` (`⌘,` on macOS) or *File → Open Settings*: a
searchable list of every option, applied as you change it. Changed
settings are marked with a bar and can be reset with ↺.

Settings live in `~/.config/qell/config.toml` (written with defaults on
first run). The Settings window saves into it in place, so your own
comments and layout are kept. You can also edit the file directly ("Edit
config.toml" in Settings): saving it in Qell applies it immediately, and a
file with a syntax error is reported and left untouched.

```toml
font_size = 14.0
tab_size = 4
auto_complete = true
ui_scale = 1.0              # whole interface, 0.75–2.0
file_icons = true
git_status = true           # colour files by git status, show the branch
git_scan_depth = 3          # folder levels searched for repositories
show_sidebar = true
show_terminal = true
shell = ""                  # e.g. "zsh -l"; empty uses $SHELL
terminal_font_size = 0.0    # 0 follows font_size
tree_hidden = [".git", ".DS_Store"]
ignore = [".git", "target", "node_modules", ".terraform", "dist"]

[lsp]
# Override a language server command:
# rust = ["rust-analyzer"]

[format]
# Override a formatter (it reads the file on stdin and writes it on stdout;
# {path} stands for the file). An empty list turns formatting off.
# terraform = ["tofu", "fmt", "-"]
```

## Architecture

```
crates/
  qell-core      text buffer (ropey), cursor, undo, tree-sitter highlighting, themes
  qell-lsp       LSP client: process + JSON-RPC framing + document sync + completion
  qell-index     workspace index: file text + trigrams for search, tree-sitter tags for definitions
  qell-term      PTY + VT emulation (alacritty_terminal) rendered as plain cell snapshots
  qell-k8s       reading a cluster through kubectl: contexts, listings, k9s-style columns
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
