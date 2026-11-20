; Highlights for JSON. The upstream query captures object keys twice
; (`@string.special.key` and `@string`) and tree-sitter-highlight keeps the
; plain string, so keys would look like values. Here no two patterns match
; the same node.

(pair key: (string) @string.special.key)
(pair value: (string) @string)
(array (string) @string)
(document (string) @string)

(number) @number

[
  (null)
  (true)
  (false)
] @constant.builtin

(escape_sequence) @escape

(comment) @comment
