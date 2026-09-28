# Notices

Omavim compiles in these tree-sitter grammars, and uses the highlight and
injection queries they ship. All are under the MIT licence; their copyright
notices are in each crate's `LICENSE` file.

| Crate | Language |
| --- | --- |
| tree-sitter | the parsing library |
| tree-sitter-md | Markdown (block and inline) |
| tree-sitter-rust | Rust |
| tree-sitter-python | Python |
| tree-sitter-javascript | JavaScript, JSX (and TypeScript's shared queries) |
| tree-sitter-typescript | TypeScript, TSX |
| tree-sitter-json | JSON |
| tree-sitter-toml-ng | TOML |
| tree-sitter-yaml | YAML |
| tree-sitter-bash | Bash |
| tree-sitter-html | HTML |
| tree-sitter-css | CSS |
| tree-sitter-php | PHP |
| tree-sitter-go | Go |
| tree-sitter-c | C |
| tree-sitter-lua | Lua |
| tree-sitter-sequel | SQL |

The text object queries in `queries/*/textobjects.scm` are from
[nvim-treesitter-textobjects](https://github.com/nvim-treesitter/nvim-treesitter-textobjects)
(rev 5c7b0263797dfd1bd6202f2b219f3b53a80b2187), under the Apache License 2.0
(`queries/LICENSE-nvim-treesitter-textobjects`). They're changed only to run
outside Neovim: `; inherits` lines are resolved by joining the files, and
`#lua-match?` is `#match?`. `tools/syntax/vendor-textobjects.py` makes them.
