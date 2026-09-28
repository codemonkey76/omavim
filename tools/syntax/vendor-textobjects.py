#!/usr/bin/env python3
"""Copy nvim-treesitter-textobjects' queries for Omavim's languages into
crates/omavim-syntax/queries/, one file per language with what it inherits
joined in, and its Lua-pattern predicates as regex ones (the patterns it
uses are the same in both).

    tools/syntax/vendor-textobjects.py PATH-TO-nvim-treesitter-textobjects
"""
import pathlib
import re
import subprocess
import sys

src = pathlib.Path(sys.argv[1]) / "queries"
root = pathlib.Path(__file__).resolve().parents[2]
out = root / "crates/omavim-syntax/queries"

# Omavim's languages, and the upstream files each is made of, in order.
LANGS = {
    "rust": ["rust"],
    "python": ["python"],
    "javascript": ["ecma", "jsx", "javascript"],
    "typescript": ["ecma", "typescript"],
    "tsx": ["ecma", "jsx", "typescript", "tsx"],
    "json": ["json"],
    "toml": ["toml"],
    "yaml": ["yaml"],
    "bash": ["bash"],
    "html": ["html"],
    "css": ["css"],
    "php": ["php"],
    "go": ["go"],
    "c": ["c"],
    "lua": ["lua"],
}

rev = subprocess.run(["git", "-C", str(src.parent), "rev-parse", "HEAD"], capture_output=True, text=True).stdout.strip()
for lang, parts in LANGS.items():
    text = f"; From nvim-treesitter-textobjects {rev} (Apache-2.0): {', '.join(parts)}\n"
    for p in parts:
        body = (src / p / "textobjects.scm").read_text()
        body = "\n".join(l for l in body.splitlines() if not l.startswith("; inherits"))
        body = re.sub(r"#(not-)?lua-match\?", lambda m: f"#{m.group(1) or ''}match?", body)
        text += f"\n; --- {p}\n{body}\n"
    d = out / lang
    d.mkdir(parents=True, exist_ok=True)
    (d / "textobjects.scm").write_text(text)
(out / "LICENSE-nvim-treesitter-textobjects").write_text((src.parent / "LICENSE").read_text())
print(f"{len(LANGS)} languages from {rev}")
