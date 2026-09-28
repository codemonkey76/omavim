# Omavim

A dead-simple writing app with Vim motions, in the spirit of
[Omawrite](https://github.com/omacom-io/omawrite), written in Rust with
[iced](https://iced.rs). See [PLAN.md](PLAN.md) for where it's going.

**Where it's up to (milestone 2):** a window in iA Writer Mono that follows the
desktop's dark/light setting, soft-wrapped lines, open/save through the
desktop's file picker, and Vim's core: normal, insert, replace and visual
(`v`, `V`) modes, counts, motions (`hjkl 0 ^ $ g_ | gg G w b e ge W B E gE
f F t T ; , { } ( ) %`), operators (`d c y > < gu gU g~` and their doubled forms,
`D C Y x X s S r J gJ ~ p P`), text objects (`iw aw iW aW is as ip ap`, the
brackets `i( i{ i[ i<` and `a` forms, the quotes `i" i' i`` and `it at`, in
visual mode too), registers (`"a`–`"z` and `"A` to append, `"0`, `"1`–`"9`,
`"-`, `"_`, the read-only `". ": "%`, `Ctrl-R` in insert mode, and `"+`/`"*`
for the desktop clipboard and primary selection), search (`/ ? n N * # g*
g#`, offsets such as `/foo/e+1`, Vim's pattern syntax, matches highlighted
as you type and after, `:noh`), moving by screen line (`gj gk g0 g^ gm g$`,
and `j`/`k` without a count go by screen line too), `H M L`, scrolling
(`CTRL-E CTRL-Y CTRL-D CTRL-U CTRL-F CTRL-B`, PageUp/PageDown, the mouse
wheel, `zt zz zb z<CR> z. z-`), marks (`m{a-z}` `m{A-Z}`, `'` and `` ` ``,
`'' '. '^ '[ '] '< '>`, kept on their text as it's edited), the jump list
(`CTRL-O`, `CTRL-I`/Tab), `gv`, `N%`, macros (`q{reg}`, `q{A-Z}` to append,
`@{reg}`, `@@`, `Q`, with a count), `CTRL-A`/`CTRL-X` (decimal, `0x` hex and `0b`
binary numbers; in visual mode too, and `g CTRL-A` for a sequence), undo/redo, `.`,
and the command line: ranges
(`N . $ % /pat/ ?pat? 'x '<,'>` with `+`/`-` offsets, `;`, `3:` and visual `:`),
`:s` and `:%s` with Vim's replacement syntax and flags (`c` asks about each
match), `:&`, `:&&`, `&`,
`g&`, `@:`, `:d :y :j :> :< :m :t :p`, `:g` and `:v` (with any of these,
`:normal` too), `:normal`, `:set` (`ic scs ws hls ts sw`), `:e`,
`:w`, `:q`, `:wq`, `:x`, `|` between commands, editing with the cursor keys,
`CTRL-W`, `CTRL-R`, `CTRL-V`, and history on Up/Down for commands and searches. Lines
wrap at word breaks and the view scrolls exactly as Neovim's does with
'linebreak' and 'smoothscroll'. `gq` and block visual mode are next.

Every Vim behaviour is checked against Neovim itself: `tools/fixtures/update`
types thousands of generated cases, key by key, into headless Neovim in a
small window, and the engine must give the same text, cursor, mode,
registers, view, marks and jump list for each.

```sh
cargo run -p omavim -- notes.md     # or no file, for a new one
```

| Keys (for now) | |
| --- | --- |
| Vim's | as in Vim |
| `Ctrl+S` | save (asks where, the first time) |
| `Ctrl+Shift+S` | save as |
| `Ctrl+Shift+O` | open |

## Install

The latest release, into `~/.local` (no Rust needed):

```sh
curl -fsSL https://github.com/codemonkey76/omavim/releases/latest/download/omavim-x86_64-linux.tar.gz | tar -xz -C ~/.local --strip-components=1
```

That puts `omavim` in `~/.local/bin` and adds it to your app launcher. It needs
`xdg-desktop-portal` (for the file pickers and dark/light), which Omarchy and most
desktops already have. To remove it:
`rm ~/.local/bin/omavim ~/.local/share/applications/omavim.desktop ~/.local/share/icons/hicolor/scalable/apps/omavim.svg`.

From source on Arch/Omarchy: `cd pkgbuild && makepkg -si`.

To release: bump `version` in Cargo.toml, commit, then
`git tag vX.Y.Z && git push origin vX.Y.Z`.

iA Writer Mono is © Information Architects Inc., under the SIL Open Font
License 1.1 (`fonts/OFL.txt`). Omavim itself is MIT.
