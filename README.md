# Qell

A minimalist code editor in Rust that looks like a terminal: near-black,
off-white, one monospace font, no decoration. One native window, a file tree, highlighted code, a terminal at the
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
- **Branches** — click the branch in the status bar (or in Source
  Control), or *Git: Switch Branch…*: local branches newest first, then
  remote ones (a remote one is checked out as a local branch tracking it);
  type a name that doesn't exist and *Create new branch* makes it where you
  are. *Git: Create Branch…*, *Delete Branch…* and *Force Delete Branch…*
  too. When git refuses (changes in the way, an unmerged branch), the
  message says what to do instead.
- **Stash** — *Git: Stash* (with an optional message), *Stash (Include
  Untracked)*, *Pop Latest Stash*, and *Pop / Apply / Drop Stash…* to pick
  one. Source Control lists each repository's stashes under *STASHES*, with
  Apply, Pop and Drop, and a **+** to stash what you have.
- **Merge conflicts** — a file git left with `<<<<<<<` / `=======` /
  `>>>>>>>` blocks (a merge, a rebase, a stash pop) shows them tinted: your
  side green, the incoming side blue, the common ancestor grey (diff3).
  Each `<<<<<<<` line ends with **Accept Current · Accept Incoming · Accept
  Both**; one click replaces the block, as one undo step. *Merge: Accept
  Current / Incoming / Both Change* work on the block around the cursor,
  *Accept All Current / Incoming* on the whole file, *Next / Previous
  Conflict* move between them. When none are left, save and stage the file.
- **Git blame** — `Ctrl/⌘+Alt+B`, *View → Git Blame*, or the palette: a
  column beside the line numbers names the author and age of the commit
  that last changed each block of lines, newer commits brighter. Hover it
  for the commit's message and date; click to copy its hash. It follows
  your edits (unsaved lines show as *not committed*) and refreshes after a
  commit.
- **Tests** — a ▶ beside every test in a test file: Go `TestX` /
  `BenchmarkX` (`*_test.go`), Rust `#[test]` (and `#[tokio::test]`),
  Jest or Vitest `it` / `test` / `describe` (`*.test.*`, `*.spec.*`; Vitest
  when package.json mentions it), pytest `test_` functions and `Test`
  classes. Click it to run that test; *Test: Run Tests in File*, *Run Test
  at Cursor* (`Ctrl/⌘+Alt+T`) and *Run Last Again* in the palette. The
  runner's own command (`go test -run '^TestSub$'`, `cargo test --exact`,
  `npx vitest run -t`, `pytest file::test`) runs in a terminal tab, so you
  see its real output, and the tab stays as a shell there afterwards. The
  ▶ turns into ✓ or ✗; the **Tests** tab beside Problems lists the results,
  failures first with their message, a click away from the failing line.
  The file is saved first; nothing ever runs on its own.
- **Daily log** — *Journal: Write Today's Log* (or *for Last Working Day*,
  for Monday's stand-up) writes what you did as a short stand-up note:
  one line, *Done* by project, *Issues*, *Next*. It is built from what Qell
  recorded that day: your commits in each repository, the files you saved,
  the tests you ran, the git commands run from Qell, and the commands you
  ran in Qell's terminals (folder, failures, long ones; `ls`, `cd`, `git
  status`… left out, repeats folded). An LLM writes the summary: the
  `[journal] command` you set (`claude -p`, `ollama run …`, `llm`; it gets
  the day on stdin), else the Anthropic API with your key, else none — the
  log then holds the activity only. It's saved as `YYYY-MM-DD.md` (the
  previous one kept as `.md.bak`) and opened rendered, the raw activity
  under the summary. The record stays on your machine (readable by you
  only, kept `keep_days`), nothing is sent until you ask for a log; secrets
  in commands (`--password x`, `TOKEN=…`, `Authorization:`, URL passwords)
  are masked, and a command typed with a leading space isn't kept (with
  `HISTCONTROL=ignorespace` in bash). The terminal hook only applies to the
  zsh or bash Qell starts — your own `.zshrc` / `.bashrc` still load.
- **Integrated terminals** — the Alacritty terminal core in a panel
  (`` Ctrl+` ``), with 256/true colour, scrollback and application-cursor mode.
  Run several shells as tabs: `` Ctrl+Shift+` `` (`⌘T` on macOS) opens a new one,
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
- **Tree order and filter** — a line of plain words under the workspace's
  name: `name` (the usual), `modified` (last changed first, with how long
  ago — a folder counts the newest change inside it, so the repository you
  touched last comes first) and `used` (opened most first; a folder counts
  what you opened in it). `changed` shows only what git sees as changed.
  Folders always come before files; the choice is kept.
- **Errors in the tree** — a file a language server reports errors in has
  a curly red line under its name, and so do the folders holding it (the
  same line marks the faulty code in the editor). Hover for the count.
  Files you haven't opened are marked when their server reports on them:
  some servers (rust-analyzer, gopls) check the whole project, others only
  the files you have open.
  Right-click a file with errors to see them (each one opens the file at
  that spot), run **Quick Fix…** on the first, or copy them; a folder lists
  its files with errors. In the editor, right-clicking a line with an
  error or warning shows the message in full, with Quick Fix and Copy.
- **Code folding** — an arrow beside the line numbers (shown when the
  pointer is over them, always on a folded line) folds the block below;
  a folded line ends with `⋯`, click it to open. `Ctrl/⌘+Alt+[` folds the
  block around the cursor, `Ctrl/⌘+Alt+]` opens it, and *Fold All /
  Unfold All* are in the Edit menu. Blocks come from indentation, so it
  works the same for every language; moving the cursor into a fold (a
  search, a jump) opens it.
- **Split editor** — `Ctrl/⌘+\` (*View → Split Editor*) puts the
  current file on both sides; open or pick any tab and it goes to the
  focused side (thin accent line on top; the other side's tab is
  underlined in grey). Click a side, or `Ctrl/⌘+1` / `Ctrl/⌘+2`, to move
  between them; drag the border to resize; `Ctrl/⌘+\` again joins them.
  The same file on both sides keeps a cursor and selection per side, and
  the split (sides, focus, width) comes back with the session.
- **Markdown preview** — `Ctrl/⌘+Alt+V`, *View → Markdown Preview*, or
  *◧ Preview* in the status bar renders a Markdown file beside its source,
  updating as you type: headings, lists, tables, task lists, quotes, code
  blocks (with a copy button) and links. Images are not loaded.
- **Unsaved changes** — closing a tab (`Ctrl/⌘+W`, its ×, or *Close
  Others / to the Right / All*) or the window with unsaved changes asks
  first: *Save*, *Don't Save* or *Cancel* (Enter saves, Esc cancels).
- **Shells come back** — with *Restore Session* on, the terminal's shells
  are started again when the workspace reopens, each in the folder it was
  in (after any `cd`), with the panel open if it was. Tabs that ran one
  command (logs, a pod's shell) aren't rerun.
- **Problems** — the bottom panel has a *PROBLEMS* tab beside the
  terminals (`Ctrl/⌘+Shift+M`, or click the error count in the status
  bar): every error and warning the language servers report, by file,
  files with errors first. Click one to go to it. In the editor, a line's
  worst problem is also written in full after its end (Settings → *Problems
  at Line End*).
- **Branches in the tree** — each repository's folder shows the branch it
  is on, dimmed after its name (Settings → *Branches in File Tree* turns
  it off). Names in the tree and tabs are set at the editor's font size.
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
- **Outline** — a section at the bottom of the Explorer lists what the open
  file defines (functions, types, methods, Terraform resources…), nested by
  indentation. The one around the cursor is highlighted and kept in view;
  click a row to go there, click the header to fold it. Read by Qell for the
  languages it parses, from the language server for the others, a moment
  after you stop typing. *Settings → Outline* hides it.
- **Sticky scroll** — while you scroll inside a function or a block, the
  lines that open it (`func …`, `for …`, `if …`) stay at the top of the
  editor, up to four. Click one to go to it. *Settings → Sticky Scroll*.
- **Signature help** — typing `(` or `,` in a call shows the function's
  signature above the cursor, the parameter being filled in underlined, with
  its documentation; it follows the cursor and closes with `Esc` or when you
  leave the call. `Ctrl/⌘+Shift+Space` opens it by hand. Needs a language
  server.
- **Inlay hints** — types and parameter names the language server infers,
  shown dimmed inside the code (`const total: number[]`, `greet(name: "hi")`);
  they aren't part of the file and the cursor steps over them. Turned on in
  the TypeScript server and gopls, which keep them off by default;
  rust-analyzer sends them as is. *Settings → Inlay Hints* hides them.
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
- **A language server per project, not per window.** Open a folder of
  projects — `~/go/src`, a monorepo — and each file gets a server rooted
  where its own project is: the nearest `go.mod` or `go.work`, `Cargo.toml`
  (the workspace one, for a workspace), `package.json`, `pyproject.toml`,
  `compile_commands.json`, and otherwise the nearest repository. Without
  that, gopls opened at `~/go/src` answers *cannot find package in GOROOT
  or GOPATH*. Qell never looks above the folder you opened, and the status
  bar says where the server was started when it isn't that folder.
- **Go to definition** — `Ctrl`/`⌘`+click a function, type, variable…
  (the symbol is underlined while the key is held), or press `F12`.
  Qell asks the language server when one is installed, and otherwise uses
  its own index (tree-sitter tags for Rust, Go, C, JavaScript, TypeScript;
  `var.`, `local.`, `module.`, `data.` and resource addresses for
  Terraform). Several matches are listed in the Search view.
  `Shift+F12` finds all references; `Alt+←` / `Alt+→` (`⌃-` / `⌃⇧-` on
  macOS) go back and forward.
- **Pictures, video and sound** open as a tab of their own: a PNG, JPEG,
  GIF, WebP, BMP or TIFF is decoded and shown fitted to the window, `1`
  for its own size and `f` back to fitted. A video shows a frame with a
  slider to move through it, the arrows stepping a second at a time (ten
  with shift), and the bar above says the size, dimensions, duration,
  frame rate and codecs; a sound file says the same of itself. `o` hands
  the file to whatever you normally open it with. Video and sound are read
  through `ffprobe` and `ffmpeg` if you have them — Qell plays nothing
  itself and installs nothing; without them it says so and what to type.
  SVG opens as code, which is more useful.
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

  `Ctrl+D` deletes what the cursor is on, after a dialog that says what it
  is about to delete and lets you choose the propagation (background,
  foreground, orphan) and whether to force it — `tab` moves, `space`
  changes, `enter` confirms, `esc` cancels. It deletes in the context the
  list is showing, and kubectl's own words come back in the status bar.

  `s` opens a shell in the pod (bash where the image has it, sh
  otherwise), `l` follows its logs and `Shift+L` the previous
  container's, and `f` forwards a port, asking which with the object's own
  port filled in. Each runs `kubectl` in a terminal tab of its own, below
  the list — so the pods stay on screen while the logs scroll, the tab is
  named for what it is doing, and Ctrl+C or closing the tab is what stops
  it. Nothing of Qell's keeps running where you can't see it.

  `d` describes the object and `y` shows its YAML, in the panel itself
  with `Esc` back to the list, so reading never takes you out of the view.
  `e` opens the manifest in an ordinary tab, since editing is the editor's
  job, and **saving that tab applies it to the cluster** — a strip above
  the text says which object and context it is bound to, and kubectl's own
  words come back in the status bar. `Esc` in that tab, or closing it,
  goes back to the list where you left it. Typing after `:` suggests resource kinds, the cluster's
  CRDs included (`Tab` takes the suggestion, `Tab` again cycles), and
  typing after `/` suggests the names in front of you.

  **Live usage.** When metrics-server is there, pods carry the columns k9s
  shows — `CPU`, `MEM`, `%CPU/R`, `%MEM/R`, `%CPU/L`, `%MEM/L`, measured
  against the pod's own requests and limits — and nodes carry theirs,
  against what they have to give. They move with the list, every two
  seconds, while the panel has the keyboard. The header draws the last
  minute of cluster CPU and memory as a sparkline, so you can see where it
  is going and not only where it is. A pod that is still coming up reads
  `Init:1/2`, and one the kernel keeps killing for memory reads
  `OOMKilled` rather than only `CrashLoopBackOff`.

  The header is k9s's: context, cluster, user, the Qell and Kubernetes
  revisions, cluster CPU and memory when metrics-server is there, the
  namespaces on `<0>`…`<9>`, and the keys — dropping a column at a time as
  the window narrows. The table carries its own frame and title,
  `Pods(web)[12]`. The helm icon under source control opens and closes it.

  The cluster view takes the window rather than sitting beside the editor:
  the file tree goes away with the editor and comes back with it. Anything
  that asks for a file — clicking Explorer, Search or Source Control,
  `Ctrl+P`, `Ctrl+B`, opening a manifest with `e` — brings the editor back,
  and `Ctrl/⌘+Shift+K` returns you to the cluster where you left it.

  It reads everything through your own `kubectl`, so it uses your
  kubeconfig, your contexts and your credential plugins (`aws eks
  get-token` and the like) — Qell holds no credentials and installs
  nothing. The list refreshes every two seconds **only while the panel has
  the keyboard**, and stops the moment you click elsewhere; the header says
  which it is doing. Nothing talks to a cluster in the background.
- **AWS view** (`Ctrl/⌘+Shift+A`, or the cloud under the helm) — the same
  kind of table for an AWS account. `:` picks the service, and `:` alone
  lists them:

  | Type | Lists | `Enter` goes into |
  | --- | --- | --- |
  | `:lambda` `:fn` | functions | details |
  | `:cloudfront` `:cf` | distributions | invalidations |
  | `:codedeploy` `:cd` | applications | latest deployments, with why a failed one failed |
  | `:codepipeline` `:cp` | pipelines | each stage and action, the stuck one in red |
  | `:codebuild` `:cb` | projects | latest builds |
  | `:s3` | buckets | folders and objects |
  | `:ec2` | instances | details |
  | `:ecs` | clusters | services, then tasks |
  | `:rds` / `:aurora` | instances / clusters | details |
  | `:iam` / `:users` | roles / users | a role's attached policies |
  | `:ssm` `:params` | Parameter Store names and types — never values | details |
  | `:stepfunctions` `:sfn` | state machines | latest executions |
  | `:sqs` | queues with their depth and dead-letter queue | details |
  | `:cognito` | user pools | app clients |
  | `:apigateway` `:api` | REST, HTTP and WebSocket APIs | stages |

  `Esc` comes back, `/` filters and `d` shows everything aws said about
  the row. `1`–`9` change region, `p` or `:profile prod` changes profile,
  and the header says which account and identity you are looking through.
  `l` follows the logs of a function or a build, and `s` opens a shell on
  an instance (Session Manager) or in an ECS task (ECS Exec), each in a
  terminal tab; `c` copies the row's ARN, `s3://` address or name.

  When a session has expired the view says so, and `i` signs in again:
  it runs `aws sso login` in a terminal tab (for the profile's whole
  `sso-session` when it has one, so every account behind it is covered),
  and reads the list again when that ends.

  It reads everything through your own `aws` CLI, so your profiles, SSO
  sessions and assumed roles are used as they are — Qell holds no
  credentials and installs nothing. Nothing is on a timer: a list is read
  when you open it and again when you press `r`. This first version only
  reads; it changes nothing in your account.

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
| AWS                   | `Ctrl+Shift+A`     |
| Toggle terminal       | `` Ctrl+` ``       |
| New terminal          | `` Ctrl+Shift+` ``, `⌘T` on macOS |
| Next / prev terminal  | `Ctrl+PageDown/Up` |
| Zoom the whole window | `Ctrl+=` / `Ctrl+-`, `Ctrl+0` resets |
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

[journal]
enabled = true              # keep a record of your day (on this machine)
shell_integration = true    # Qell's zsh/bash terminals report their commands
command = ""                # LLM fed the day on stdin: "claude -p", "ollama run llama3.1", "llm"
api_key = ""                # else the Anthropic API (empty: $ANTHROPIC_API_KEY)
model = "claude-haiku-5-5"
language = "English"        # the log's language
dir = ""                    # where logs go; empty: <data>/qell/journal
keep_days = 60              # days of activity kept
```

## Architecture

```
crates/
  qell-core      text buffer (ropey), cursor, undo, tree-sitter highlighting, themes
  qell-lsp       LSP client: process + JSON-RPC framing + document sync + completion
  qell-index     workspace index: file text + trigrams for search, tree-sitter tags for definitions
  qell-term      PTY + VT emulation (alacritty_terminal) rendered as plain cell snapshots
  qell-k8s       reading a cluster through kubectl: contexts, listings, k9s-style columns
  qell-aws       reading an AWS account through the aws CLI: profiles, regions, listings
  qell-table     the table both views list: columns, rows, ages and sizes
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
