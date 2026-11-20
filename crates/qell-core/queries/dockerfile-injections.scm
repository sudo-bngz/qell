; RUN / CMD / ENTRYPOINT in shell form are shell commands. Children
; (fragments, line continuations) are part of the command.
((shell_command) @injection.content
  (#set! injection.language "shellscript")
  (#set! injection.include-children))
