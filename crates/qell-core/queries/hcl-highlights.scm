; Highlights for HCL / Terraform. The tree-sitter-hcl crate ships no queries,
; so this is Qell's own, kept deliberately small.

(comment) @comment
(string_lit) @string
(quoted_template_start) @string
(quoted_template_end) @string
(template_literal) @string
(heredoc_template) @string
(heredoc_identifier) @string.special
(numeric_lit) @number
(bool_lit) @constant.builtin
(null_lit) @constant.builtin

(block (identifier) @keyword)
(block (string_lit) @type)
(attribute (identifier) @property)
(function_call (identifier) @function)
(get_attr (identifier) @property)
(variable_expr (identifier) @variable)

[ "for" "in" "if" "else" "endif" "endfor" ] @keyword
[ "{" "}" "[" "]" "(" ")" ] @punctuation.bracket
[ "=" "==" "!=" "<" ">" "<=" ">=" "+" "-" "*" "/" "%" "&&" "||" "!" "?" ":" "=>" ] @operator
[ "," "." ] @punctuation.delimiter
(template_interpolation_start) @punctuation.special
(template_interpolation_end) @punctuation.special
