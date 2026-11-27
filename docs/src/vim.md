# Vim mode

Turn it on in **Settings → Editor → Vim Mode**, or with *View: Toggle Vim
Mode* in the palette. The editor gets Normal, Insert and Visual modes and a
command line under the code; the file tree, tabs, terminal and the rest
work as usual.

## Motions

`h j k l`, `w b e`, `0 ^ $`, `gg G`, `{ }`, `%`, `f t F T ; ,`,
`Ctrl-D` / `Ctrl-U`, with counts (`5j`, `3w`).

## Operators

- `d c y > <` with motions, on lines (`dd`, `3yy`, `>>`) and on text
  objects (`ciw`, `di(`, `ca"`, `yi{`).
- `x D C s S J r ~ p P`, undo `u` and redo `Ctrl-R`, repeat `.`.

## Visual mode

`v` for characters, `V` for lines; any operator then works on the
selection.

## Search

`/` and `?` search forward and back (regular expressions, highlighted),
`n` / `N` repeat, `*` searches the word under the cursor.

## Commands

| | |
|---|---|
| `:w` `:q` `:wq` `:x` `:wa` `:qa` | write and quit |
| `:e file` | open a file |
| `:42` | go to line 42 |
| `:bn` `:bp` | next / previous tab |
| `:s/a/b/g`, `:%s/a/b/g` | substitute (also on a selection) |
| `:d` | delete lines |
| `:noh` | clear the search highlight |
| `:set wrap` / `:set nowrap` | word wrap |
| `:term` | open a terminal |
| `:Ex` | show the file tree |

`gd` goes to the definition, `gt` / `gT` switch tabs.
