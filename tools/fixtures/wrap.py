#!/usr/bin/env python3
"""Writes the soft-wrap test cases (tests/fixtures/wrap_cases.json): lines and
row widths. tools/fixtures/wrap.lua records where Neovim ('linebreak' on)
puts each char on screen; omavim-vim's wrap::layout is tested against that.
"""
import json, pathlib, random
from cases import TEXTS

random.seed(7)
WORDS = ["a", "an", "the", "quick", "brown", "fox", "jumps", "over", "lazy", "dog", "extraordinary",
         "well-known", "e.g.", "a/b", "x+y", "why?", "yes!", "semi;colon", "and,", "end.", "@home",
         "日本語", "テキスト", "café", "naïve", "supercalifragilisticexpialidocious", "--", "..."]
LINES = [line for text in TEXTS.values() for line in text.split("\n")]
for _ in range(120):
    words = random.choices(WORDS, k=random.randint(3, 16))
    seps = random.choices([" ", " ", " ", "  ", "\t", " \t", "   "], k=len(words))
    LINES.append("".join(w + s for w, s in zip(words, seps)).rstrip() if random.random() < 0.7
                 else "".join(w + s for w, s in zip(words, seps)))
WIDTHS = [7, 10, 13, 20, 30, 41]

if __name__ == "__main__":
    root = pathlib.Path(__file__).resolve().parents[2]
    cases = [{"line": l, "width": w} for l in LINES for w in WIDTHS]
    path = root / "crates/omavim-vim/tests/fixtures/wrap_cases.json"
    path.write_text(json.dumps(cases, ensure_ascii=False) + "\n")
    print(f"{len(cases)} wrap cases → {path.relative_to(root)}")
