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

# A text long enough to scroll in the 30x8 window the cases run in: lines of
# many lengths (one taller than the window), blank lines, indents, a tab.
_WORDS = ("the quick brown fox jumps over a lazy dog while it was bright and cold "
          "in april and the clocks were striking thirteen").split()
_LONG = []
for i in range(40):
    if i % 7 == 3:
        _LONG.append("")
    elif i == 16:
        _LONG.append(" ".join(_WORDS[(j * 5) % len(_WORDS)] for j in range(60)))
    else:
        n = (i * 7) % 23 + 1
        words = " ".join(_WORDS[(i + j) % len(_WORDS)] for j in range(n))
        _LONG.append(("    " if i % 5 == 1 else "\t" if i == 12 else "") + words + ".")
TEXTS["long"] = "\n".join(_LONG)

CURSORS = {
    "prose": [(0, 0), (0, 10), (1, 30), (3, 17), (4, 6), (5, 8)],
    "code": [(0, 0), (1, 12), (1, 16), (2, 4), (3, 0), (6, 9)],
    "edge": [(0, 0), (1, 0), (2, 2), (3, 1), (4, 3)],
    "unicode": [(0, 3), (1, 6), (1, 13), (2, 1)],
    "long": [(0, 0), (6, 10), (16, 100), (20, 3), (39, 5)],
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

SEARCH = [
    "/o<CR>", "/o<CR>n", "/o<CR>N", "/o<CR>nn", "?o<CR>", "?o<CR>n", "?o<CR>N", "3/o<CR>", "/o<CR>3n",
    "/the<CR>", "/\\<the\\><CR>", "/e<CR>", "/^<CR>", "/^<CR>n", "/$<CR>", "/$<CR>n", "?$<CR>",
    "?^<CR>", "/zzz<CR>", "/<CR>", "n", "/e/e<CR>", "/e/e<CR>n", "/e/e+1<CR>", "/e/e-1<CR>n",
    "/e/s+1<CR>", "/o/s+1<CR>n", "/e/b-1<CR>", "/e/+1<CR>", "/e/-<CR>", "/e/+1<CR>n", "?e?e<CR>",
    "/o<CR>/<CR>", "/o<CR>//e<CR>", "/o/e<CR>/<CR>", "/[aeiou]\\{2}<CR>", "/\\(ow\\|he\\)<CR>",
    "/o\\+<CR>", "/.*<CR>", "/\\cthe<CR>", "/\\s\\+<CR>", "/\\d<CR>", "/[,.;]<CR>",
    "/\\a\\a\\a\\a\\a<CR>", "*", "#", "g*", "g#", "2*", "*n", "#n", "*N", "**", "##",
    "d/o<CR>", "d/e/e<CR>", "c/o<CR>X<Esc>", "y/o<CR>", "d?o<CR>", "dn", "/o<CR>dn", "/o<CR>d2n",
    "/o<CR>dN", "d*", "d#", "y/e/+1<CR>", "d/^<CR>", "d/$<CR>", "v/o<CR>", "v/o<CR>d", "v?o<CR>y",
    "V/e<CR>d", "vnd", "/o<CR>x.n.", "d/o<CR>u", "/o<BS>e<CR>", "/ox<C-u>e<CR>", "/<BS>x", "/o<Esc>x",
]

EX = [
    ":s/o/0/<CR>", ":s/o/0/g<CR>", ":%s/o/0/g<CR>", ":%s/the/THE/gi<CR>", ":%s/The/X/I<CR>",
    ":s/\\(\\w\\+\\) \\(\\w\\+\\)/\\2 \\1/<CR>", ":s/ /\\r/g<CR>", ":%s/$/;/<CR>",
    ":%s/^/# /<CR>", ":s/x*/-/g<CR>", ":%s/e/&&/g<CR>", ":s/o/\\U&/g<CR>", ":%s/\\<\\w/\\u&/g<CR>",
    ":%s/\\w\\+/\\L\\u&/g<CR>", ":s/\\w\\+/\\U&\\E!/<CR>", ":2,3s/a/A/g<CR>", ":.,$s/e/E/<CR>",
    ":.,.+1s/e/E/g<CR>", ":s/e/E/g 3<CR>", ":%s/zzz/y/<CR>", ":%s/zzz/y/e<CR>", ":s/o/0/<CR>j:s<CR>",
    ":s/o/0/<CR>j&", ":s/o/0/g<CR>j:&&<CR>", ":s/o/0/<CR>g&", "/the<CR>:s//X/<CR>", "*:s//X/g<CR>",
    ":s/o/X/<CR>:s/e/~Y/<CR>", ":%s/\\n/ /g<CR>", ":%s#e#/#g<CR>", ":%s/o/0/n<CR>",
    ":s/o/0/<CR>u", ":%s/o/0/g<CR>u", ":%s/o/0/g<CR>u<C-r>", ":s/o/0/<CR>n", ":%s/[aeiou]/\\t/g<CR>",
    ":3<CR>", ":$<CR>", ":.+2<CR>", ":-<CR>", ":/the/<CR>", ":?the?<CR>", ":/e/+1<CR>", ":2;+1s/e/E/<CR>",
    ":%d<CR>", ":2,3d<CR>", ":d<CR>", ":d 2<CR>", ":d a<CR>", ":2,3y<CR>", ":y a<CR>", ":.,$y<CR>",
    ":2,3j<CR>", ":j<CR>", ":j!<CR>", ":j 3<CR>", ":2,3><CR>", ":><CR>", ":>><CR>", ":<<CR>", ":> 2<CR>",
    ":2,3m0<CR>", ":m$<CR>", ":m+1<CR>", ":m-2<CR>", ":1t.<CR>", ":2,3co$<CR>", ":t0<CR>", ":m0<CR>u",
    "Vj:s/e/E/g<CR>", "Vj:d<CR>", "Vj:><CR>", "vj:s/o/0/<CR>", "Vj<Esc>G:'<,'>d<CR>", "3:d<CR>",
    "2:s/e/E/<CR>", ":set ic<CR>/THE<CR>", ":set ts=4<CR>j", ":set sw=2<CR>>>", ":set nows<CR>G/the<CR>",
    ":s/o/0/<CR>:<Up><CR>", ":%s/e/E/<CR>u:<Up><CR>", "yiw:s/<C-r>\"/X/g<CR>", ":s/xo<Left><BS><End>/0/<CR>",
    ":s/o/0/<Esc>", ":s/o/0<BS><BS><BS><BS><BS><BS>", ":s/abc<C-w>o/0/<CR>", ":s/abc<C-u>s/o/0/<CR>",
    "/o<CR>/e<CR>/<Up><Up><CR>", ":s/o/0/<CR>@:", ":1,2d|3d<CR>",
]

MARKS = [
    "ma", "majj'a", "majj`a", "majjd'a", "majjd`a", "jmakdd'a", "jmakk2dd`a", "majdd'a", "jjmakJ`a",
    "jjmakdd`a", "maxu`a", "maddu'a", "jmakddu'a", "mAjj'A", "majjmbgg:'a,'bd<CR>", "ma`b", "G''",
    "G``", "Ggg``", "G<C-o>", "G<C-o><C-i>", "Ggg<C-o><C-o>", "3G5G<C-o><C-o>", "/the<CR><C-o>",
    "n<C-o>", "}<C-o>", "}}<C-o><C-o>", "H<C-o>", "L''", "*<C-o>", "G<C-o><Tab>", "Gm'gg<C-o>",
    "ywP'[", "ywP`]", "yy`]", "yj`]", "x'.", "x`.", "dd'.", "iabc<Esc>'^", "iabc<Esc>`^", "iabc<Esc>`[",
    "iabc<Esc>`]", "Aabc<Esc>`[", "vjj<Esc>'<", "vjj<Esc>`>", "Vjj<Esc>`>", "vjj<Esc>gv", "vjj<Esc>ggvgv",
    "Vj<Esc>jgvd", ">>`[", ">j`]", "gUw`]", ":s/o/0/<CR>`[", "Gdgg''", "maGdgg'a",
]

# Screen-line motions and scrolling, run on "long" from several views.
VIEW = [
    "gj", "gk", "3gj", "3gk", "g0", "g^", "gm", "g$", "2g$", "g$gj", "g$gk", "$gj", "$gk", "gjgj",
    "H", "M", "L", "3H", "3L", "dH", "yL", "dM", "dgj", "ygk",
    "<C-e>", "<C-y>", "3<C-e>", "3<C-y>", "10<C-e>", "10<C-y>", "<C-e><C-e><C-e><C-e>",
    "<C-d>", "<C-u>", "<C-d><C-d>", "<C-u><C-u>", "2<C-d>", "2<C-u>", "2<C-d><C-d>",
    "<C-f>", "<C-b>", "<C-f><C-f>", "<C-b><C-b>", "2<C-f>", "<PageDown>", "<PageUp>",
    "zt", "zz", "zb", "z<CR>", "z.", "z-", "z+", "z^", "5zt", "30zz", "20zb", "jzt", "kzb",
    "G", "gg", "20G", "}}}", "{{", "30j", "30k", "/april<CR>", "?the<CR>", "Gdd", "ggdd",
    "vjjj<C-e>", "Vjjjzz", "ix<Esc><C-d>",
]
VIEW_STARTS = [  # (cursor, top) in "long"
    ((0, 0), None), ((8, 2), (6, 0)), ((16, 50), (16, 0)), ((16, 200), (16, 5)),
    ((19, 0), (16, 7)), ((25, 4), (20, 0)), ((39, 3), (33, 0)),
]

SENTENCES = [")", "(", "2)", "3(", "d)", "d(", "y2)", "c)X<Esc>"]

# :g, :v and :normal.
GLOBAL = [
    ":g/the/d<CR>", ":v/e/d<CR>", ":g!/e/d<CR>", ":g/o/s/o/0/g<CR>", ":g/^$/d<CR>", ":g/^/m0<CR>",
    ":g/e/t$<CR>", ":g/e/normal Ax<CR>", ":g/e/norm! 0x<CR>", ":%norm Ax<CR>", ":normal dw<CR>",
    ":2,3norm x<CR>", ":g/e/j<CR>", ":g/a/<CR>", ":g/zzz/d<CR>", ":v/zzz/d<CR>",
    ":g/e/s/e/E/|s/a/A/<CR>", ":g/e/><CR>", ":2,$g/e/d<CR>", ":g//d<CR>", "/o<CR>:g//d<CR>",
    ":g/e/normal A<C-v><Esc>x<CR>", ":g/e/normal ix<CR>u", ":g/e/d<CR>u", ":g/e/d<CR><C-o>",
    ":g/e/normal dd<CR>", ":g/e/v/a/d<CR>", ":g/e/normal jdd<CR>", ":g/e/p<CR>", ":g/e/d<CR>n",
    ":g/e/normal cwX<CR>", ":g/e/normal vd<CR>", ":g/e/normal d<CR>", ":g/e/norm :s/e/Q/<C-v><CR>x<CR>",
    "x:g/e/d<CR>.", ":normal Ax<CR>.", ":g/./normal ma<CR>'a", ":g/e/normal @q<CR>", "qqAx<Esc>q:g/e/normal @q<CR>",
    ":g/e/norm $<C-a><CR>",
]

# :s///c, answering its questions.
CONFIRM = [
    ":s/e/E/c<CR>y", ":s/e/E/gc<CR>yny", ":%s/e/E/gc<CR>yyy<Esc>", ":%s/e/E/c<CR>a", ":%s/e/E/gc<CR>nnna",
    ":%s/e/E/gc<CR>l", ":%s/e/E/gc<CR>nl", ":%s/e/E/gc<CR><Esc>", ":%s/e/E/gc<CR>nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn<Esc>",
    ":%s/e/E/gc<CR>yyu", ":%s/o/0/gc<CR>yny<Esc>", ":%s/\\n/ /gc<CR>yy<Esc>", ":%s/\\n/ /gc<CR>ny<Esc>", ":%s/x*/-/gc<CR>yyy<Esc>",
    ":%s/zzz/E/c<CR>", ":%s/e/E/gc<CR>y<C-e>y<Esc>", ":%s/e/&&/gc<CR>yy<Esc>", ":%s/e/E/c<CR>yyy<Esc>", ":%s/e/E/c<CR>ya",
    ":%s/e/E/gc<CR>yy<Esc><C-o>", ":%s/e/E/gc<CR>y<Esc>u<C-r>", ":2,3s/a/A/gc<CR>yyyy<Esc>", ":%s/e/E/gc<CR>yy<Esc>'[", ":%s/e/E/gc<CR>yy<Esc>`]",
    ":%s/e/E/gc<CR>yx<Esc>",
]

# gq and gw, on every text and on one made for them.
FORMAT = [
    "gqq", "gqj", "gqip", "gqap", "gq}", "gqgq", "gww", "gwip", "Vjgq", "vjgq", "gqG", "gggqG", "gqqu",
    "gqip.", "gwap`[", "gqip`]", "magqip`a", "3gqq", "gqk", "gqip<C-o>", "gwj", "gqqj", "Vgw",
]
FORMAT_TEXT = (
    "This is a paragraph with enough words that it will need to be wrapped at the text width.\n"
    "And a second line of it.\n"
    "\n"
    "    An indented paragraph that is also long enough to be broken into lines.\n"
    "// A comment line that is long enough to wrap, with its leader repeated.\n"
    "// And another comment line.\n"
    "# A hash comment that goes on and on past the width.\n"
    "> A quoted line that runs past the width of the window here.\n"
    "> > Nested quote that is long as well, to see the leaders.\n"
    "- A list item that is long enough to wrap onto the next line.\n"
    "/* A block comment that starts here and runs on past the width */\n"
    "/* A block comment that runs past the width\n"
    " * with a middle line\n"
    " */\n"
    ".PP\n"
    "Troff paragraph after a macro line.\n"
    "supercalifragilisticexpialidocious_and_more_words_without_breaks okay\n"
    "\tTab indented line that is long enough to wrap at the width.\n"
    "        Eight spaces of indent on a line that wraps.\n"
    "x"
)
FORMAT_CURSORS = [(0, 5), (1, 3), (3, 8), (4, 10), (5, 0), (6, 4), (7, 3), (8, 5), (9, 2), (10, 3),
                  (11, 6), (12, 2), (14, 0), (15, 3), (16, 10), (17, 2), (18, 9), (19, 0)]

# CTRL-A and CTRL-X, on a text of numbers, from several places.
NUMBERS_TEXT = (
    "x 007 -5 0x1f 0XAB 0b101 -0x10 42abc end\n"
    "18446744073709551615 -18446744073709551615 9 -1 0 0b0 a-3\n"
    "no numbers here\n"
    "\n"
    "v1.2.3 0x 0b2 --7 x-1 0xfF 0x0FF 0b0011\n"
    "  10\n"
    "20 30\n"
    "40"
)
NUMBER_CURSORS = [(0, 0), (0, 3), (0, 6), (0, 11), (0, 15), (0, 21), (0, 26), (0, 33), (1, 0), (1, 21),
                  (1, 45), (1, 51), (2, 3), (3, 0), (4, 0), (4, 7), (4, 13), (4, 16), (4, 21), (4, 27),
                  (5, 0), (6, 3)]
NUMBERS = [
    "<C-a>", "<C-x>", "5<C-a>", "5<C-x>", "100<C-x>", "<C-a>.", "<C-x>3.", "<C-a>u", "<C-a>w<C-a>",
    "<C-a>`[", "<C-a>`]", "<C-a>j", "10<C-a>", "11<C-x>",
    "v<C-a>", "vj<C-a>", "Vj<C-a>", "Vjj<C-x>", "vjjg<C-a>", "Vjjjg<C-a>", "Vjjj2g<C-x>", "v$<C-a>",
    "ve<C-a>", "vl<C-x>", "Vj<C-a>.", "vjg<C-a>u", "Vjj<C-a>`[", "Vjj<C-a>`]", "v3l<C-a>",
]

# Macros: recording (what the register holds) and running them.
MACROS = [
    "qaxjq@a", "qaxjq2@a", "qaxjq@au", "qaxxq@a@@u", "qadwjq@a", "qa0xjq5@a", "qaxfZxq@a",
    "qa<Esc>xjq@a", "qaiab<Esc>jq@a", "qaq", "qbyyq", "qAxq", "qaxqqAjq@a", "qaxq\"ap",
    "qa:s/e/E/<CR>jq@a", "qaxq@b", "qaxqQ", "qaxq2Q", "qa3xq", "qa/o<CR>q@a", "qaix<BS>y<Esc>q@a",
    "qaxqu@a", "qaxj.q@a", "qaxjq@a.", "qaxjq@au<C-r>", "qaA!<Esc>jq3@a", "qawq@a", "qadwq\"ap",
    "qa3jq@a", "qaddq2@a", "qaGq@a", "qaxq@A", "qajq100@a", "qbxqqa@bjq@a", "qaxjq@@", "@@",
    "Q", "qa\"byyq\"bp", "qqxjq@q", "yyq1xq@1", "qavjdq@a", "qaVjyq@a", "qa3q@a", "qa\"aq",
    "qa<C-d>q@a", "qa}q@a", "qa>>jq@a", "qaciwX<Esc>wq@a", "qaxjq@au@a",
    "Vy", "Vjy", "Vky", "vjy", "Vjyu",
]


def cases():
    out = []
    keys = (
        MOTIONS
        + [f"{op}{m}" for op, m in itertools.product(["d", "y"], OPERATOR_MOTIONS)]
        + [f"c{m}X<Esc>" for m in OPERATOR_MOTIONS]
        + [f"{op}{m}" for op, m in itertools.product([">", "gU", "g~"], ["w", "j", "}", "$"])]
        + ["2d3w", "3d2w", "2y2j", "c2wX<Esc>"]
        + SIMPLE + INSERT + VISUAL + SENTENCES + REGISTERS + SEARCH + EX + MARKS + MACROS + GLOBAL + CONFIRM + FORMAT
        + [f"{op}{o}" for op, o in itertools.product(["d", "y", "gU"], OBJECTS)]
        + [f"c{o}X<Esc>" for o in OBJECTS]
        + [f"v{o}" for o in OBJECTS]
        + ["diw.", "daw..", "ci(X<Esc>j.", "vawd", "vipd", "vi(y", "viwiw", "vawaw", "vi(i(", "va(a(",
           "vjiw", "vipip", "vitit", "vlaw"]
    )
    for name, text in TEXTS.items():
        for (line, col), k in itertools.product(CURSORS[name], keys):
            out.append({"name": f"{name}@{line},{col}: {k}", "text": text, "cursor": [line, col], "keys": k})
    # The marks that change with every edit ('[ '] '. '^ '< '>) are
    # compared in the mark cases only.
    for case in out:
        if case["keys"] in MARKS:
            case["all_marks"] = True
    for (line, col), k in itertools.product(FORMAT_CURSORS, FORMAT):
        case = {"name": f"format@{line},{col}: {k}", "text": FORMAT_TEXT, "cursor": [line, col], "keys": k}
        if "`" in k:
            case["all_marks"] = True
        out.append(case)
    for (line, col), k in itertools.product(NUMBER_CURSORS, NUMBERS):
        case = {"name": f"numbers@{line},{col}: {k}", "text": NUMBERS_TEXT, "cursor": [line, col], "keys": k}
        if "`" in k:
            case["all_marks"] = True
        out.append(case)
    for ((line, col), top), k in itertools.product(VIEW_STARTS, VIEW):
        case = {"name": f"long@{line},{col}^{top}: {k}", "text": TEXTS["long"], "cursor": [line, col], "keys": k}
        if top:
            case["top"] = list(top)
        out.append(case)
    return out


if __name__ == "__main__":
    root = pathlib.Path(__file__).resolve().parents[2]
    path = root / "crates/omavim-vim/tests/fixtures/cases.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    data = cases()
    path.write_text(json.dumps(data, ensure_ascii=False, indent=0) + "\n")
    print(f"{len(data)} cases → {path.relative_to(root)}")
