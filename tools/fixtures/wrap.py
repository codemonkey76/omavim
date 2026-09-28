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

# 'breakindent' (as Omavim wraps), on indented lines, some of them code, and
# code's 'breakat' (without "-", so `->` stays whole). Their own random
# numbers, so the cases above stay as they were.
CODE_BREAKAT = " \t!@*+;:,./?"
_r = random.Random(11)
CODE = ["$this->send($settings,", "$to,", "$subject,", "=>", "match", "($x)", "{", "}", "fn",
        "foo(a,", "b);", "return", "self.value", "x", "-=", "->", "'smtp'", "//", "comment",
        "if", "(a && b)", "throw new \\RuntimeException(\"x\");"]
INDENTS = ["", "  ", "    ", "        ", "\t", "\t\t", "  - ", "    1. ", "            ",
           "                        ", "\t    "]
INDENTED = []
for _ in range(80):
    words = _r.choices(WORDS + CODE, k=_r.randint(4, 20))
    INDENTED.append(_r.choice(INDENTS) + " ".join(words))
INDENTED += ["        'mailgun'      => $messageId = $this->sendViaMailgun($settings, $to, $subject, $body, $cc),",
             "    " + "a" * 70, "\t" + "word " * 30, "  - a list item that goes on for quite a long way past the edge"]
BRI_WIDTHS = [20, 25, 30, 41, 60, 80]

if __name__ == "__main__":
    root = pathlib.Path(__file__).resolve().parents[2]
    cases = [{"line": l, "width": w} for l in LINES for w in WIDTHS]
    cases += [{"line": l, "width": w, "breakindent": True} for l in INDENTED for w in BRI_WIDTHS]
    cases += [{"line": l, "width": w, "breakindent": True, "breakat": CODE_BREAKAT}
              for l in INDENTED for w in BRI_WIDTHS]
    path = root / "crates/omavim-vim/tests/fixtures/wrap_cases.json"
    path.write_text(json.dumps(cases, ensure_ascii=False) + "\n")
    print(f"{len(cases)} wrap cases → {path.relative_to(root)}")
