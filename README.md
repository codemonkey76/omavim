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
wheel, `zt zz zb z<CR> z. z-`), undo/redo, `.`, and the basic `:` commands
(`:w [file]`, `:q`, `:q!`, `:wq`, `:x`, `:N`). Lines wrap at word breaks and
the view scrolls exactly as Neovim's does with 'linebreak' and
'smoothscroll'. Marks, macros and the rest of `:` are next.

Every Vim behaviour is checked against Neovim itself: `tools/fixtures/update`
types thousands of generated cases, key by key, into headless Neovim in a
small window, and the engine must give the same text, cursor, mode,
registers and view for each.

```sh
cargo run -p omavim -- notes.md     # or no file, for a new one
```

| Keys (for now) | |
| --- | --- |
| Vim's | as in Vim |
| `Ctrl+S` | save (asks where, the first time) |
| `Ctrl+Shift+S` | save as |
| `Ctrl+O` | open (until Vim's jump list claims it) |

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
