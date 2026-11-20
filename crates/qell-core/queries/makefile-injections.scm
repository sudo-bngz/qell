; Recipe lines are shell commands. `$(VAR)` references inside them stay
; excluded from the injection, so make still colours those.
((recipe_line (shell_text) @injection.content)
  (#set! injection.language "shellscript"))
