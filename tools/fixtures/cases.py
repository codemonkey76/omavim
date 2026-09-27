#!/usr/bin/env python3
"""Writes the Vim behaviour test cases (tests/fixtures/cases.json).

Each case is a starting text, a cursor and some keys. tools/fixtures/nvim.lua
runs them through Neovim to record what should happen; omavim-vim is tested
against that. Regenerate both with tools/fixtures/update.
"""
import itertools, json, pathlib

TEXTS = {
    "prose": (
        "The quick brown fox jumps over the lazy dog.\n"
        "It was a bright, cold day in April; and the clocks were striking thirteen.\n"
        "\n"
        "A new paragraph (with brackets) and \"quotes\" here.\n"
        "    indented line with\ttab and more-words_here\n"
        "last line"
    ),
    "code": (
        "fn main() {\n"
        "    let x = foo(a, b[1], {c: 2});\n"
        "    if x > 3 { return; }\n"
        "}\n"
        "\n"
        "// comment line\n"
        "struct S { a: i32 }"
    ),
    "edge": "a\n\nb c\n  \nend.",
    "unicode": "café crème brûlée\nnaïve résumé — déjà vu\n日本語 text",
    "markup": (
        "<div class=\"a\">\n"
        "  <p>Hello <b>big</b> world. Second sentence!  Third one?</p>\n"
        "  <span>it's 'single' and `tick` and \"dq \\\" esc\"</span>\n"
        "</div>\n"
        "\n"
        "End (nested (parens) here) [x]. Next (\n"
        "  multi\n"
        ") done."
    ),
}

CURSORS = {
    "prose": [(0, 0), (0, 10), (1, 30), (3, 17), (4, 6), (5, 8)],
    "code": [(0, 0), (1, 12), (1, 16), (2, 4), (3, 0), (6, 9)],
    "edge": [(0, 0), (1, 0), (2, 2), (3, 1), (4, 3)],
    "unicode": [(0, 3), (1, 6), (1, 13), (2, 1)],
    "markup": [(0, 2), (0, 12), (1, 8), (1, 14), (1, 30), (1, 45), (2, 10), (2, 22),
               (2, 36), (2, 44), (3, 0), (4, 0), (5, 9), (5, 16), (5, 30), (6, 3)],
}

MOTIONS = ["h", "j", "k", "l", "0", "^", "$", "g_", "5|", "gg", "G", "2G", "+", "-", "_",
           "<CR>", "w", "W", "b", "B", "e", "E", "ge", "gE", "fe", "Fe", "te", "Te",
           "fe;", "fe;,", "te;", "{", "}", "%", "3w", "2b", "3e", "2j", "3k", "2$", "10l", "10h"]

OPERATOR_MOTIONS = ["w", "W", "b", "e", "E", "ge", "$", "0", "^", "j", "k", "l", "h", "gg", "G",
                    "}", "{", "%", "fe", "te", "2w", "3l", "2j"]

SIMPLE = [
    "x", "3x", "X", "2X", "D", "2D", "C<Esc>", "Y", "yy", "2yy", "dd", "2dd", "3dd", "cc<Esc>",
    "ccnew<Esc>", "S<Esc>", "s", "sZ<Esc>", "3sZ<Esc>", "rZ", "3rZ", "20rZ", "J", "3J", "gJ", "2gJ",
    "~", "4~", "yyp", "yyP", "ywP", "yw2p", "ddp", "xp", "Yp", "2yyjp", ">>", "2>>", "<<", ">j",
    "guu", "gUU", "g~~", "gUw", "g~$", "dw.", "x..", "2x.", "dw3.", "dd.", "cwX<Esc>w.",
    ">>j.", "ddu", "dwu", "dwdwu", "dwuu", "dw2u", "xu<C-r>", "dwdwuu<C-r>", "iabc<Esc>u",
    "3xu", "ddjddu", "Jux",
]

INSERT = [
    "iX<Esc>", "aX<Esc>", "IX<Esc>", "AX<Esc>", "oX<Esc>", "OX<Esc>", "3ix<Esc>", "2aab<Esc>",
    "3oline<Esc>", "i<BS><BS>X<Esc>", "a<BS><Esc>", "A<CR>new<Esc>", "o<Esc>", "i<CR><Esc>",
    "iab<C-w>c<Esc>", "A foo bar<C-w><Esc>", "ione<CR>two<Esc>", "A<CR><CR>x<Esc>", "iX",
    "Rxyz<Esc>", "Rxy<BS><Esc>", "3Rab<Esc>", "Rlonger than the line is<Esc>", "R<BS><BS>q<Esc>",
    "aX<Esc>.", "oX<Esc>j.", "3iab<Esc>.",
]

VISUAL = [
    "vwd", "vjd", "Vjd", "vey", "veyP", "v$d", "vwcX<Esc>", "vjy", "v3l~", "vwU", "vwu", "Vj>",
    "vwod", "vj>", "vw", "Vj", "v2e", "vwo", "vjoky", "vwv", "vwV", "Vv", "v$y", "vggd", "VGd",
    "vjJ", "Vjc<Esc>", "vwp", "yevep", "vw<Esc>", "vjd.", "vwdu",
]


OBJECTS = ["iw", "aw", "iW", "aW", "is", "as", "ip", "ap", "i(", "a)", "ib", "i{", "aB", "i[", "a]",
           "i<", "a>", 'i"', 'a"', "i'", "a'", "i`", "a`", "it", "at", "2iw", "3aw", "2i(", "2a(",
           "2it", "2is", "2ap"]

REGISTERS = [
    '"ayy', '"ayw"Ayy', '"Ayy', '"ayw"Ayw', '"ayy"Ayw', '"ayy"ap', '"ayyj"byy"ap"bP', 'dd"1p',
    'dddd"2p', 'dw"-p', 'yyjdd"0p', 'dddddd"1p..', 'dddddd"1pu.u.', '"_dd', '"_ddp', 'yw"_dwP',
    '"add', '"adw', 'x"-p', '"ax"ap', 'd}', 'd%', 'd)', '3"ayy', '"a3yy', '2"a3yy',
    '"adw.', 'v"ay', 'viw"ay"ap', 'yiwviw"_dP', 'yyvj"ap', 'ciwX<Esc>"-p', 'cwnew<Esc>"1p',
    'ddu"1p', 'yyp"0p', '"Add', 'i<C-r>"<Esc>', 'yiwA <C-r>0<Esc>', 'yyo<C-r>0<Esc>',
    'dwi<C-r>-<Esc>', '"ayiwo<C-r>a<Esc>', '"ayiwo<C-r>a<Esc>j.', 'dd"1p"2p', 'dddd"1p"2p',
    'dd"ap', '"1P', '"xyy"Xyy"xp', 'yiwjviwp"-p', 'yiwjviwpp', 'Vjd"1p', 'vjd"1p', 'vwd"-p',
    '"a"byy', '""yy', 'yw""p', '"Ayw"Ayy"ap', 'x"Ax"ap', 'cc<Esc>"1p', '"_x"-p',
]

SENTENCES = [")", "(", "2)", "3(", "d)", "d(", "y2)", "c)X<Esc>"]


def cases():
    out = []
    keys = (
        MOTIONS
        + [f"{op}{m}" for op, m in itertools.product(["d", "y"], OPERATOR_MOTIONS)]
        + [f"c{m}X<Esc>" for m in OPERATOR_MOTIONS]
        + [f"{op}{m}" for op, m in itertools.product([">", "gU", "g~"], ["w", "j", "}", "$"])]
        + ["2d3w", "3d2w", "2y2j", "c2wX<Esc>"]
        + SIMPLE + INSERT + VISUAL + SENTENCES + REGISTERS
        + [f"{op}{o}" for op, o in itertools.product(["d", "y", "gU"], OBJECTS)]
        + [f"c{o}X<Esc>" for o in OBJECTS]
        + [f"v{o}" for o in OBJECTS]
        + ["diw.", "daw..", "ci(X<Esc>j.", "vawd", "vipd", "vi(y", "viwiw", "vawaw", "vi(i(", "va(a(",
           "vjiw", "vipip", "vitit", "vlaw"]
    )
    for name, text in TEXTS.items():
        for (line, col), k in itertools.product(CURSORS[name], keys):
            out.append({"name": f"{name}@{line},{col}: {k}", "text": text, "cursor": [line, col], "keys": k})
    return out


if __name__ == "__main__":
    root = pathlib.Path(__file__).resolve().parents[2]
    path = root / "crates/omavim-vim/tests/fixtures/cases.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    data = cases()
    path.write_text(json.dumps(data, ensure_ascii=False, indent=0) + "\n")
    print(f"{len(data)} cases → {path.relative_to(root)}")
