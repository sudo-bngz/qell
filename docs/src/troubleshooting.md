# Troubleshooting

## macOS says Qell "can't be opened"

The app isn't signed by Apple yet. Right-click `Qell.app` → **Open**, then
**Open** — once. Or:

```sh
xattr -dr com.apple.quarantine /Applications/Qell.app
```

## "LSP not installed"

Qell never installs language servers. Hover the message for the command
to run (click copies it), install it, and reopen the file. If the server
is somewhere unusual, give its path under `[lsp]` in the
[configuration](configuration.md#language-servers).

Started from the Dock, Qell takes the `PATH` of your login shell. If a tool
is found in a terminal but not in Qell, check that your `PATH` is set in
`~/.zprofile` or `~/.zshrc` (not only in a terminal app's settings).

## gopls says "cannot find package"

Qell starts a server at each project's root (the nearest `go.mod`,
`Cargo.toml`, `package.json`…), never above the folder you opened. Open the
project's folder, or a folder containing it — not a folder inside it.

## Qell crashed

Nothing is lost:

- every few seconds, tabs with unsaved changes are copied aside; the next
  start puts them back in their tabs, **still unsaved**, for you to look at
  and save;
- a crash writes a report — the message, where it happened and a
  backtrace — and the next start tells you where it is.

Reports are in `~/Library/Application Support/qell/crash/` on macOS and
`~/.local/share/qell/crash/` on Linux. Please
[open an issue](https://github.com/sudo-bngz/qell/issues/new) with it.

When a bug breaks a single frame, Qell doesn't close: it tells you
*Something went wrong*, keeps your work backed up, and carries on. It
stops only if that keeps happening.

## Logs

Start Qell from a terminal with `RUST_LOG` to see what it does:

```sh
RUST_LOG=qell=debug qell ~/src/project
```

## Reset everything

Settings and sessions are in `config.toml` and `session.json`, in
`~/Library/Application Support/qell/` on macOS and `~/.config/qell/` on
Linux — delete `config.toml` to go back to the defaults. The activity
journal, backups and crash reports are in Qell's data folder: the same
folder on macOS, `~/.local/share/qell` on Linux.
