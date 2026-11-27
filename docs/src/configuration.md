# Configuration

Open **Settings** with `Ctrl/⌘+,` (or *File → Open Settings*): a searchable
list of every option, applied as you change it. Changed options are marked
with a bar; ↺ puts one back to its default.

Settings live in a TOML file, written with the defaults the first time Qell
starts:

- macOS: `~/Library/Application Support/qell/config.toml`
- Linux: `~/.config/qell/config.toml`

The Settings window saves into it **in place**, keeping your comments and
layout. You can also edit the file itself (*Edit config.toml* in Settings):
saving it in Qell applies it at once, and a file with a syntax error is
reported and left alone.

## Editor

| Key | Default | |
|---|---|---|
| `font_size` | `14.0` | Editor font size, in points |
| `tab_size` | `4` | Spaces inserted by Tab, and the width of a tab |
| `auto_close` | `true` | Close brackets and quotes as you type; wrap a selection |
| `format_on_save` | `true` | Format the file when it's saved, when a formatter is available |
| `auto_complete` | `true` | Ask for completions while typing, not only on `Ctrl+Space` |
| `vim_mode` | `false` | [Vim](vim.md) keys in the editor |
| `word_wrap` | `false` | Wrap long lines at the window's width |
| `minimap` | `true` | The file in miniature on the right |
| `bracket_pair_colors` | `true` | Colour brackets by depth |
| `indent_guides` | `true` | Lines at each indentation level |
| `outline` | `true` | The Outline section under the file tree |
| `sticky_scroll` | `true` | Keep the lines opening the current blocks at the top |
| `inlay_hints` | `true` | Show the language server's inlay hints |
| `inline_problems` | `true` | Write an error's message after the end of its line |

## Window and files

| Key | Default | |
|---|---|---|
| `ui_scale` | `1.0` | Scale of the whole interface (0.75–2.0) |
| `show_sidebar` | `true` | Show the file tree on startup |
| `show_terminal` | `true` | Show the terminal panel on startup |
| `restore_session` | `true` | Reopen tabs, the split and the terminals |
| `file_icons` | `true` | File-type icons in the tree and tabs |
| `shortcuts` | `6` | Folders and files pinned above the tree (0: none) |
| `shortcuts_recent` | `false` | Order them by last opened rather than by use |
| `tree_sort` | `"name"` | `"name"`, `"modified"` or `"used"` |
| `tree_changed` | `false` | Show only what git sees as changed |
| `tree_hidden` | `[".git", ".DS_Store"]` | Names hidden from the tree |
| `ignore` | `[".git", "target", "node_modules", ".terraform", "dist"]` | Skipped by quick open and search |

## Git

| Key | Default | |
|---|---|---|
| `git_status` | `true` | Colour files by git status, show the branch |
| `git_scan_depth` | `3` | Folder levels searched for repositories (0: only the workspace's own) |
| `tree_branches` | `true` | Show each repository's branch beside its folder |

## Terminal and tools

| Key | Default | |
|---|---|---|
| `shell` | `""` | Command for new terminals, e.g. `"zsh -l"`; empty uses `$SHELL` |
| `terminal_font_size` | `0.0` | Terminal font size; 0 follows `font_size` |
| `kubectl` | `"kubectl"` | The kubectl the [cluster view](kubernetes.md) runs |
| `aws` | `"aws"` | The aws CLI the [AWS view](aws.md) runs |

## Language servers

`[lsp]` replaces the built-in server command for a language (see
[Languages](languages.md)):

```toml
[lsp]
rust = ["rust-analyzer"]
go = ["/home/me/go/bin/gopls"]
terraform = ["terraform-ls", "serve"]
```

Language ids: `rust`, `go`, `typescript`, `javascript`, `c`, `cpp`,
`terraform`, and the names of the other highlighted languages.

## Formatters

Formatting uses the language server when it can format; otherwise the
language's own tool, if it's on your `PATH`:

| Language | Tool |
|---|---|
| Rust | `rustfmt` |
| Go | `gofmt` |
| Terraform | `terraform fmt` |
| Python | `black` |
| C, C++ | `clang-format` |
| Shell | `shfmt` |
| TOML | `taplo format` |
| JavaScript, TypeScript, JSON, CSS, HTML, YAML, Markdown | `prettier` |

`[format]` sets your own. The command reads the file on stdin and writes
the result on stdout; `{path}` stands for the file. An empty list turns
formatting off for that language:

```toml
[format]
terraform = ["tofu", "fmt", "-"]
markdown = []
```

A formatter gets a few seconds; one that fails or hangs never blocks the
save.

## Daily log

`[journal]` — see [Daily log](journal.md):

```toml
[journal]
enabled = true              # keep a record of your day, on this machine
shell_integration = true    # Qell's zsh/bash terminals report their commands
command = ""                # the LLM, fed the day on stdin: "claude -p", "ollama run llama3.1"
api_key = ""                # else the Anthropic API (empty: $ANTHROPIC_API_KEY)
model = "claude-haiku-5-5"
language = "English"        # the log's language
dir = ""                    # where logs go; empty: Qell's data folder
keep_days = 60              # days of activity kept
```
