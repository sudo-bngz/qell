# Files and search

## The file tree

The Explorer (`Ctrl/⌘+Shift+E`) shows the folder you opened, with an icon
and colour per file type and git's state on every file.

- **Shortcuts** — above the tree sit the folders you work in most (the
  repositories, in a folder that holds several), then the files you open
  most. Pin the ones you always want there; *MOST USED* can switch to
  *RECENT*.
- **Order and filter** — the words under the workspace's name: `name`,
  `modified` (last changed first, with how long ago — a folder counts the
  newest change inside it) and `used` (opened most first). `changed` shows
  only what git sees as changed. The choice is kept.
- **Branches** — each repository's folder shows the branch it's on.
- **Errors** — a file with errors has a curly red line under its name, and
  so do the folders holding it.
- **Right-click** for New File and New Folder, Rename and Delete in place
  (Delete moves to the Trash after asking), Reveal in Finder, Open in
  Integrated Terminal and Copy Path.

Qell handles big workspaces — a `~/src` with hundreds of repositories: the
index is updated file by file as things change, not walked again.

## Go to file

`Ctrl/⌘+P` searches the whole workspace by name as you type (`crt` finds
`api/cart.go`); `Enter` opens it in the focused side of the editor.

## Search in files

`Ctrl/⌘+Shift+F` searches every file of the workspace. The workspace is
indexed in memory (respecting `.gitignore` and `ignore` in the
[configuration](configuration.md)), so results come as you type, even in
large trees. Toggles for case, whole word and regular expressions; click a
match to open it there.

**Replace across files** — the chevron in the Search view opens a replace
box. *Replace All* asks once, then changes every match. Files open in a
tab are changed in the editor and left unsaved, so you can read them over
and undo; the others are written to disk.

## Go to symbol

`Ctrl/⌘+Shift+O` lists what the open file defines, to type into:
functions, types and methods, and Terraform blocks as `local.env` or
`aws_s3_bucket.logs`.

## Go to definition without a server

When no language server is installed, `F12` and `Ctrl`/`⌘`+click use
Qell's own index: tree-sitter tags for Rust, Go, C, JavaScript and
TypeScript, and `var.`, `local.`, `module.`, `data.` and resource addresses
for Terraform. Several matches are listed in the Search view.
