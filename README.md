# Omavim

A dead-simple writing app with Vim motions, in the spirit of
[Omawrite](https://github.com/omacom-io/omawrite), written in Rust with
[iced](https://iced.rs). See [PLAN.md](PLAN.md) for where it's going.

**Milestone 1 (now):** a window in iA Writer Mono that follows the desktop's
dark/light setting, plain typing, and open/save through the desktop's file
picker. Vim arrives in milestone 2.

```sh
cargo run -p omavim -- notes.md     # or no file, for a new one
```

| Keys (for now) | |
| --- | --- |
| `Ctrl+S` | save (asks where, the first time) |
| `Ctrl+Shift+S` | save as |
| `Ctrl+O` | open |

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
