# Terminal

The panel at the bottom (`` Ctrl+` ``) runs your shells, on the Alacritty
terminal core: 256 colours and true colour, scrollback, mouse selection.

- **Tabs** — `` Ctrl+Shift+` `` (`⌘T` on macOS) opens another shell;
  `Ctrl+PageDown` / `Ctrl+PageUp` move between them; `exit` closes one.
- **Copy and paste** — select with the mouse (double-click: a word,
  triple-click: a line), copy with `⌘C` / `Ctrl+Shift+C`, paste with `⌘V` /
  `Ctrl+Shift+V`. Right-click for Copy, Paste, Select All and Clear.
- `Ctrl+C` interrupts, as in any terminal.
- **Shells come back** — with *Restore Session* on, the shells start again
  in the folders they were in when you reopen the workspace.
- The bottom panel also holds **PROBLEMS** (errors and warnings) and, once
  tests have run, **TESTS**.

## Your shell

New terminals run `$SHELL`, or the command in `shell` (e.g. `"zsh -l"`) in
the [configuration](configuration.md). Your `.zshrc` / `.bashrc` load as
usual.

When the [daily log](journal.md) is on, the zsh or bash Qell starts gets a
small hook that notes each command, its folder, exit status and duration,
on your machine only. Turn it off with `shell_integration = false` under
`[journal]`; shells you open outside Qell are never touched.

## Commands that open terminals

The cluster view's shell, logs and port-forward, the AWS view's logs,
shells and sign-in, and the [test runner](tests.md) each run in a terminal
tab of their own, named for what they do — so you see the real output, and
closing the tab is what stops them.
