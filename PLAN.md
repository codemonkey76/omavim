# Omavim — plan

A dead-simple writing app, like [Omawrite](https://github.com/omacom-io/omawrite),
written in **Rust** with **iced**, where **Vim motions** are how you edit. Markdown
first, and code too: highlighted with **tree-sitter**, with Vim text objects that
understand it.

It stays a focused editor for one file at a time, not an IDE. The line is drawn
under *Scope*.

Omawrite's README is the starting point for what the app does. Its keyboard
shortcuts aren't: Omavim has its own, built for a Vim user (see *Keys*).

## What Omawrite does (from its README), and what Omavim does instead

| Omawrite | Omavim |
| --- | --- |
| Qt Quick and C++ | Rust and iced (see *Choices*) |
| Follows system dark/light mode, live | Same |
| Text size follows `omarchy display text size` / GNOME `text-scaling-factor`, re-flows without a restart; 12px is the design size | Same |
| iA Writer Mono, bundled (SIL OFL 1.1) | Same font, same licence file |
| Save, save as and open, through the portal's file picker | Same, from `:w` `:saveas` `:e` and the leader keys |
| Print through the system print dialog | Same, through the portal's print dialog; printed at the right size (Omawrite's issue #29, tiny print, is designed out) |
| New window | Same |
| Undo and redo | Vim's: `u`, `Ctrl-r`, `U` |
| Fullscreen | Same |
| Search, next, previous | `/`, `?`, `n`, `N`, `*`, `#` |
| Find and replace | `:s` and `:%s` with Vim's syntax |
| Insert bold, italic, link | Leader keys; in visual mode they wrap the selection |
| Shortcut reference | Same, for Omavim's keys and Vim's, and `:help` |
| Recovers unsaved drafts after an abnormal exit | Same |
| Watches open files; warns before an outside change replaces local work | Same |
| Needs `xdg-desktop-portal` and a backend | Same |
| Markdown only | Markdown and code: syntax highlighting for the bundled languages, and code blocks in Markdown highlighted in their own language |

## Vim: what "fully support" means

Vim mode is the editor, not an add-on. It's measured against real Vim: every
behaviour below has tests whose expected results come from running the same keys
in Neovim (see *Testing*).

**Modes:** normal, insert, replace (`R`), visual (`v`), visual line (`V`),
visual block (`Ctrl-v`), operator-pending, command-line (`:`, `/`, `?`).
The cursor shows the mode (block, bar, underline) and so does the footer.

**Counts** on everything that takes one: `3w`, `2dd`, `5.`, `d3w`, `3d2w`.

**Motions**
- Characters and lines: `h j k l`, `0 ^ $ g_`, `gg G`, `{count}G`, `|`, `+ - _`, `Enter`
- Words: `w W b B e E ge gE`
- Find in line: `f F t T ; ,`
- Text: `( )` sentences, `{ }` paragraphs, `%` matching brackets
- Screen: `H M L`, `Ctrl-d Ctrl-u Ctrl-f Ctrl-b Ctrl-e Ctrl-y`, `zz zt zb`
- **Wrapped lines**: `gj gk g0 g^ g$`, and an option (on by default, because this
  is a prose editor) for `j`/`k` to move by screen line
- Search: `/ ? n N * # g* g#`
- Marks and jumps: `m{a-z}`, `` ` `` and `'`, `` `` ``, `''`, `Ctrl-o`, `Ctrl-i`, `` `. `` `` `^ ``, `` `[ `` `` `] ``, `` `< `` `` `> ``

**Operators:** `d c y`, `> <`, `=` (reflow paragraph), `gu gU g~`, `gq`/`gw`
(format to the column width), `!` is left out (see *Not in scope*).
Doubled forms (`dd cc yy >> gUU`), and `D C Y`, `x X s S`, `r`, `J gJ`, `~`,
`Ctrl-a Ctrl-x` on numbers.

**Text objects:** `iw aw iW aW`, `is as`, `ip ap`, `i" a" i' a' i` a``,
`i( a( ib ab`, `i[ a[`, `i{ a{ iB aB`, `i< a<`, `it at`.
From tree-sitter, on top:
- Markdown: `i* a*` (emphasis), `il al` (link), `ih ah` (heading section), `ic ac`
  (fenced code block)
- Code: `if af` (function), `ik ak` (class, struct or impl), `ia aa` (argument),
  `i/ a/` (comment), in every language whose grammar has the queries for them
- Moving by them: `]f [f` (next/previous function), `]k [k` (class), `]h [h`
  (Markdown heading)

**Insert mode:** `i a I A o O gi`, `Ctrl-w Ctrl-u Ctrl-h`, `Ctrl-r {register}`,
`Ctrl-o {command}`, `Ctrl-t Ctrl-d` (indent list items).

**Visual mode:** all motions and operators; `o O gv`; block `I` and `A`;
`r` and `~` on selections.

**Registers:** unnamed, `"a-z` (and `"A-Z` to append), `"0`, `"1-"9`, `"-`,
`"_`, `"+`/`"*` (the Wayland clipboard), `".`, `":`, `"/`, `"%`.

**Repeat and macros:** `.` (with counts), `q{register}` / `@{register}` / `@@`.

**Undo:** `u`, `Ctrl-r`, `U`; one insert session is one undo step, as in Vim.

**Command line:** `:w :q :wq :x :q! :e :e! :saveas :new :print :help`
(and the leader keys below),
`:s :%s :'<,'>s` with flags `g i c`, ranges (`.`, `$`, `%`, `'a`, `N,M`, `/pat/`),
`:noh`, `:set` for the few settings Omavim has, `:{number}`, history on `↑ ↓`.
`@:` repeats the last command.

**Settings** in `~/.config/omavim/config.toml`: start mode (normal or insert),
`j`/`k` by screen line, `ignorecase`/`smartcase`, and a switch to turn Vim off
entirely for people who want Omawrite's plain editing.

**Not in scope, at least at first:** Vimscript, plugins, `:!` and `!` filters,
windows and splits, tabs, folds, digraphs, spell checking, `.vimrc`.

## Scope: a writing app that also edits code, not an IDE

In: syntax highlighting, the tree-sitter text objects above, the language chosen by
the file's extension (`:set filetype=` to override), indentation that follows the
language on `o`/`O` and after `{`, and `%` and bracket matching that ignore brackets
in strings and comments.

**Out**, deliberately, because past this line Omavim becomes another Helix or Zed,
which are years of work by teams: language servers (completion, go to definition,
diagnostics), a file tree, splits and tabs, project-wide search, a built-in terminal,
plugins, Git integration.

## Keys

Vim's keys mean what they mean in Vim. Omavim's own actions sit on a **leader
key, `Space`**, in normal mode, as in many Vim setups, so nothing clashes:

| Keys | Does | Also |
| --- | --- | --- |
| `Space w` | save (picker if it's never been saved) | `:w`, and `Ctrl+S` in every mode |
| `Space W` | save as | `:saveas` |
| `Space o` | open | `:e` (picker), `:e name` |
| `Space n` | new window | `:new` |
| `Space p` | print | `:print` |
| `Space f` | fullscreen | `:fullscreen` |
| `Space b` / `Space i` / `Space l` | bold / italic / link: on the word, or around the selection in visual mode | |
| `Space ?` | the key reference | `:help` |
| `Space q` | close the window (asks if unsaved) | `:q` |

`Ctrl+S` is the one Ctrl shortcut, kept in every mode because muscle memory
expects it and Vim doesn't use it. The leader and its keys can be changed in the
config file.

## Choices

**iced.** The interface is one text column, a quiet footer and the odd prompt, so
a full desktop toolkit isn't needed, and iced is pure Rust (the COSMIC desktop is
built with it). It renders text with `cosmic-text`, which does the shaping, wrapping
and font fallback a prose editor needs. What GTK or Qt would have given for free,
Omavim adds through **xdg-desktop-portal**, using the `ashpd` crate:

| Need | How |
| --- | --- |
| Open and save pickers | the FileChooser portal |
| Printing | lay pages out ourselves into a PDF (in points, at the page's size), then hand it to the Print portal, which shows the system dialog |
| Dark/light, live | the Settings portal's `color-scheme`, watched |
| Text size, live | the Settings portal's `text-scaling-factor`, watched. **To check early:** what `omarchy display text size` actually changes |
| Clipboard (`"+`) | iced's clipboard, which speaks Wayland |

**The editor is Omavim's own widget.** iced's built-in `text_editor` is for plain
editing: its cursor, selection and key handling aren't ours to shape. A Vim editor
needs a block cursor per mode, visual line and block selections, and every key.
So the app has an editor widget written on iced's widget API, drawing a
`cosmic-text` buffer: the text with its highlighting, the cursor, the selections,
the search matches, the command line.

**Vim is its own crate, with no GUI.** `omavim-vim` turns keys into edits and
cursor moves on a `TextModel` trait (read text, find positions, grapheme and word
boundaries, apply an edit, report screen lines for wrapped text). It holds modes,
counts, registers, marks, the jump list, macros, the command line and undo
grouping. The app implements `TextModel` over its buffer; the tests implement it
over `ropey`. `unicode-segmentation` handles graphemes and words.

**Tree-sitter** for highlighting and structure, in its own crate, `omavim-syntax`:
- The `tree-sitter` crate, one grammar crate per language, and the highlight,
  text-object and indent query files, taken from Helix and nvim-treesitter
  (both MIT/Apache: keep their notices).
- **Incremental:** every edit is passed to the parser as an edit, so re-parsing
  after a keystroke touches only what changed. Highlighting covers only what's on
  screen.
- **Injections:** a fenced code block in Markdown is parsed and highlighted with
  its own language's grammar.
- **Markdown styling** in Omawrite's spirit: headings larger or bolder, emphasis in
  italic and bold, links and code set apart, markup characters dimmed. The same tree
  keeps `gq` from breaking a link or reflowing a code block.
- **Colours:** tree-sitter's highlight names (`keyword`, `string`, `function`,
  `comment`, `markup.heading`, …) map to a colour scheme with a dark and a light
  version. Ideally the scheme comes from the current Omarchy theme; to find out
  early what Omarchy themes provide.
- **Bundled languages** to start: Markdown, Rust, Python, JavaScript, TypeScript,
  JSON, TOML, YAML, Bash, HTML, CSS, PHP, Go, C, Lua, SQL. Each grammar adds a few
  hundred KB to a few MB to the binary; loading grammars at runtime (as Helix does,
  for hundreds of languages) is a later step if it's ever wanted.
- A language with no grammar opens as plain text, with Vim working as usual.

**Files:** a file watcher (`notify`) for outside changes; drafts written to
`~/.local/share/omavim/drafts/` a second or two after typing stops and when the
window loses focus, removed on save or a clean close, offered back at start after a
crash. Settings in `~/.config/omavim/config.toml`.

**The risk with iced** is that its editor pieces are younger than GTK's or Qt's:
input methods, and accessibility, are the weakest parts. Neither matters much for
one person writing English prose, but they're why GTK was the other option.

## Layout

```
omavim/
  Cargo.toml          workspace
  crates/
    omavim-vim/       the Vim engine (no GUI), and its tests
    omavim-syntax/    tree-sitter: languages, highlighting, text objects, indentation
    omavim/           the app: iced window, the editor widget, files, portals, printing
  fonts/              iA Writer Mono + OFL.txt
  data/               desktop file and icon
  pkgbuild/           PKGBUILD for Arch / Omarchy
  runtime/queries/    highlight, text-object and indent queries per language
  tests/fixtures/     Vim behaviour fixtures, generated (see Testing)
```

## Testing

- **Vim behaviour is checked against real Vim.** A generator script feeds each test
  case (starting text, cursor, keys) to `nvim --headless --clean`, and records the
  resulting text, cursor, mode and registers as a fixture. `omavim-vim` must produce
  the same. Hundreds of cases, many generated (every motion × every operator × with
  and without counts, text objects at edges, empty lines, Unicode). This is how
  "full Vim motions" gets proven rather than claimed.
- Tree-sitter: highlighting snapshot tests per language (a sample file and the
  expected highlight names), text-object tests per language, and a check that
  re-parsing after an edit matches a fresh parse.
- Unit tests for the leader keys.
- App tests for files: save and reopen, draft recovery, an outside change prompting.
- A print test: the PDF Omavim makes has 12pt text at A4, not 1.5pt.

## Milestones

1. **Skeleton.** Workspace, an iced window with the font and the editor widget
   showing text, dark/light following, open and save through the portal, the
   PKGBUILD. Build and tests in CI.
2. **The editor widget and the Vim core.** Typing, wrapping, scrolling, the cursor
   per mode; modes, counts, motions, operators, `.`, undo grouping; the fixture
   generator and the first few hundred cases passing. From here it's usable.
3. **The rest of Vim.** Text objects (the plain ones), registers and the
   clipboard, marks and jumps, macros, search, the command line and `:s`,
   wrapped-line movement, `gq`.
4. **Tree-sitter.** The syntax crate, incremental parsing, highlighting with the
   dark and light schemes, Markdown styling, code blocks in Markdown, the bundled
   languages, the Markdown and code text objects and `]f`-style moves, indentation.
5. **Omawrite's features.** Text size, print, new window, fullscreen, key
   reference, the leader keys, draft recovery, outside-change watching, the
   unsaved-changes prompt.
6. **Everyday use.** A README, packaging, using it for a while and fixing what grates.

(Vim comes before Omawrite's features this time: with iced there's no ready-made
editor to lean on, so the editor and Vim are built together from the start.)

## Open questions

- Normal or insert mode when a document opens? (Setting either way; which default?)
- `Space` as leader: fine, or another key?
- What does `omarchy display text size` change, and can the Settings portal see it?
- Can the colour scheme come from the current Omarchy theme, and switch with it?
- Which languages to bundle, beyond the list above?
- Public from the start (GitHub, MIT like Omawrite), or private until it's usable?
- Name: *omavim* is taken from the brief; check nothing in the Omarchy world already
  uses it.
