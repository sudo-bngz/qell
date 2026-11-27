# Local history

Each time Qell saves a file, it keeps a copy — on your machine, whether the
file is in git or not. So you can look back at what a file was ten minutes
ago, or this morning, and bring it back.

The first copy is the file as it was on disk **before Qell first wrote over
it**, so the history starts where your edits did. A file changed by
something else (a `git checkout`, a formatter, another editor) is kept as it
was before Qell saves over it too. Saving without changes keeps nothing new.

## Looking back

**Local History: Show File History…** lists the versions of the open file,
newest first, with when they were kept and how they differ from the file now
(`+3 −1 since then`, or *same as now*).

Pick one: it opens in a tab beside the file, as an inline diff — **red** for
the lines that version had, **green** for what the file has now.

## Restoring

In that tab, **Local History: Restore Version…** puts the file back as it
was. From any other tab it lists the versions to pick from first.

The file's text is replaced **in the editor, unsaved**, as one undo step:
look it over, then save — or Undo, and nothing happened.

## Where it's kept

In Qell's data folder, under `history/`
(`~/Library/Application Support/qell/history` on macOS,
`~/.local/share/qell/history` on Linux), in files only you can read. Nothing
is sent anywhere.

Versions are kept for 30 days, up to 100 per file; files over 1 MB aren't
kept. Change it, or turn it off, under `[history]` in the
[configuration](configuration.md#local-history).
