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

To install on Arch/Omarchy: `cd pkgbuild && makepkg -si`.

iA Writer Mono is © Information Architects Inc., under the SIL Open Font
License 1.1 (`fonts/OFL.txt`). Omavim itself is MIT.
