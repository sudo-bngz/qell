; Qell additions to the tree-sitter-make highlights, appended after the
; upstream query. When two patterns capture the same node the later one
; wins, so every target (standard ones like `all` included) is a function.

; Every rule target, like VS Code's entity.name.function.
(targets (word) @function)
