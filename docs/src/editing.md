# Editing

## Cursors and selections

- **Several cursors** — `Ctrl/⌘+D` selects the word under the cursor, then
  adds the next occurrence each time; `Ctrl/⌘+Shift+L` selects them all.
  `Ctrl+Alt+↑` / `↓` (`⌥⌘↑` / `↓` on macOS) adds a cursor above or below.
  `Alt`+click (`⌥`+click) adds one where you click, or removes it. `Esc` goes back to one.
- Typing, deleting, completion and paste work at every cursor.
- Double-click selects a word, triple-click a line; drag to extend.

## Typing helpers

- **Brackets and quotes close themselves** as you type; with a selection,
  typing `(` or `"` wraps it.
- **Toggle comment** — `Ctrl/⌘+/` comments or uncomments the lines the
  cursors are on, in the language's own comment style.
- **Auto-indent** keeps the indentation of the line above and adds a level
  after an opening bracket.
- **Format** — `Shift+Alt+F`, and on save when *Format on Save* is on.
  Qell uses the language server when it can format, or a formatter you set
  (see [Configuration](configuration.md#formatters)). A formatter that fails
  or hangs never blocks the save: the file is saved as it is and the
  message says why.

## Find and replace

`Ctrl/⌘+F` opens a find bar over the editor, `Ctrl+H` (`⌥⌘F` on macOS) with
replace. Matches are highlighted as you type; `Enter` / `F3` go to the next,
`Shift+Enter` / `Shift+F3` to the previous. Toggles for case, whole word and
regular expressions sit in the bar. To search the whole workspace, see
[Files and search](files.md).

## Seeing the code

- **Code folding** — an arrow beside the line numbers (shown when the
  pointer is over them) folds the block below the line; a folded line ends
  with `⋯`. `Ctrl/⌘+Alt+[` folds the block around the cursor and
  `Ctrl/⌘+Alt+]` opens it; *Fold All* and *Unfold All* are in the palette.
  Blocks are found by indentation, so it works the same in every language.
- **Sticky scroll** — while you scroll inside a function or a block, the
  lines that open it stay at the top of the editor. Click one to go there.
- **Outline** — the bottom of the Explorer lists what the file defines,
  nested, with the one around the cursor highlighted. Click to jump.
- **Minimap** — the whole file in miniature on the right; click or drag to
  move. Hidden on narrow editors.
- **Bracket pairs and indent guides** — brackets are coloured by depth and
  the pair around the cursor is boxed; guides mark each indentation level.
- **Word wrap** — `Alt+Z` (`⌥Z` on macOS). Wrapped lines keep their
  indentation.
- **Tabs** are drawn to the next tab stop (`tab_size`), and clicks, the
  cursor and Up/Down follow them.

## Split editor

`Ctrl/⌘+\` splits the editor in two, with the current file on both sides.
Tabs you click or files you open go to the **focused** side — it has a thin
line along its top; the tab shown on the other side is underlined in grey.
Click a side, or press `Ctrl/⌘+1` / `Ctrl/⌘+2`, to move between them; drag
the border to resize; `Ctrl/⌘+\` again joins them.

With the same file on both sides, each side keeps its own cursor and
selection. The split comes back with the session.

## Markdown

`Ctrl/⌘+Alt+V` (or ◧ *Preview* in the status bar) shows a Markdown file
rendered beside its source, updated as you type.

## Pictures, video and sound

A PNG, JPEG, GIF, WebP, BMP or TIFF opens as a picture, fitted to the tab —
`1` shows it at its own size, `f` fits it again. A video shows a frame with
a slider to move through it (the arrows step a second, ten with Shift), and
the bar above gives its size, dimensions, duration and codecs; a sound file
says the same of itself. `o` opens the file in your usual app. Video and
sound are read through `ffprobe` and `ffmpeg` if you have them. SVG opens
as code.

## Files changed elsewhere

When a `git checkout`, a `terraform fmt` or another program changes an
open file, Qell reloads it quietly if you hadn't touched it. If you had
unsaved edits, a bar asks: *Reload* (undo brings yours back) or *Keep
mine*. Saving never silently overwrites a change made elsewhere.

## Unsaved work

- Closing a tab or the window with unsaved changes asks first: *Save*,
  *Don't Save* or *Cancel*.
- Every few seconds, tabs with unsaved changes are copied aside. If Qell
  ever crashes, the next start puts them back — still unsaved, for you to
  look at and save. See [Troubleshooting](troubleshooting.md).
