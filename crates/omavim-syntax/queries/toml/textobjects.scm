; From nvim-treesitter-textobjects 5c7b0263797dfd1bd6202f2b219f3b53a80b2187 (Apache-2.0): toml

; --- toml
[
  (integer)
  (float)
] @number.inner

(table
  (pair) @parameter.inner @parameter.outer)

(inline_table
  "," @parameter.outer
  .
  (_) @parameter.inner @parameter.outer)

(inline_table
  .
  (_) @parameter.inner @parameter.outer
  .
  ","? @parameter.outer)

(array
  "," @parameter.outer
  .
  (_) @parameter.inner @parameter.outer)

(array
  .
  (_) @parameter.inner @parameter.outer
  .
  ","? @parameter.outer)

(comment) @comment.outer
