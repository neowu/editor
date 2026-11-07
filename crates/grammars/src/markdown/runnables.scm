; Run a shell code block's own code, with the icon on its opening fence
((fenced_code_block
  (info_string
    (language) @run)
  (code_fence_content) @code)
  (#any-of? @run "sh" "shell")
  (#set! tag markdown-sh))

((fenced_code_block
  (info_string
    (language) @run)
  (code_fence_content) @code)
  (#eq? @run "bash")
  (#set! tag markdown-bash))

((fenced_code_block
  (info_string
    (language) @run)
  (code_fence_content) @code)
  (#eq? @run "zsh")
  (#set! tag markdown-zsh))

((fenced_code_block
  (info_string
    (language) @run)
  (code_fence_content) @code)
  (#eq? @run "fish")
  (#set! tag markdown-fish))
