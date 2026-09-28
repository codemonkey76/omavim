; From nvim-treesitter-textobjects 5c7b0263797dfd1bd6202f2b219f3b53a80b2187 (Apache-2.0): bash

; --- bash
(function_definition) @function.outer

(function_definition
  body: (compound_statement
    .
    "{"
    _+ @function.inner
    "}"))

(case_statement) @conditional.outer

(if_statement
  (_) @conditional.inner) @conditional.outer

(for_statement
  (_) @loop.inner) @loop.outer

(while_statement
  (_) @loop.inner) @loop.outer

(comment) @comment.outer

(regex) @regex.inner

((word) @number.inner
  (#match? @number.inner "^[0-9]+$"))

(variable_assignment) @assignment.outer

(variable_assignment
  name: (_) @assignment.inner @assignment.lhs)

(variable_assignment
  value: (_) @assignment.inner @assignment.rhs)

(command
  argument: (word) @parameter.inner)
