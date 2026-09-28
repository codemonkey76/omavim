; From nvim-treesitter-textobjects 5c7b0263797dfd1bd6202f2b219f3b53a80b2187 (Apache-2.0): css

; --- css
[
  (integer_value)
  (float_value)
  (color_value)
] @number.inner

(declaration
  (property_name) @assignment.lhs
  .
  ":"
  _+ @assignment.inner @assignment.rhs
  ";") @assignment.outer

(declaration
  (property_name) @assignment.inner)

(comment) @comment.outer
