//! The key reference (`Space ?`, `:help`): Omavim's own keys, as the config
//! file has them, then Vim's.

use crate::config::{Action, Config};

/// The reference's scroller, to scroll from the keys.
pub const ID: &str = "help";

/// A section: its title, and (keys, what they do) rows.
pub type Section = (&'static str, Vec<(String, String)>);

pub fn sections(config: &Config) -> Vec<Section> {
    let leader = config.leader_name();
    let mut own: Vec<(String, String)> = Action::ALL
        .iter()
        .filter_map(|&a| {
            let key = config.key_for(a)?;
            let also = a.command().map(|c| format!("   {c}")).unwrap_or_default();
            Some((format!("{leader} {key}"), format!("{}{also}", a.describe())))
        })
        .collect();
    own.push(("Ctrl+S".into(), "save, in every mode".into()));
    let mut all = vec![("Omavim", own)];
    all.extend(VIM.iter().map(|(title, rows)| {
        (
            *title,
            rows.iter()
                .map(|(k, d)| (k.to_string(), d.to_string()))
                .collect(),
        )
    }));
    all
}

/// Vim's keys, in brief.
const VIM: &[(&str, &[(&str, &str)])] = &[
    (
        "Modes",
        &[
            (
                "i a I A o O",
                "insert: before, after, line start, line end, new line below, above",
            ),
            ("R", "replace"),
            ("v V Ctrl-V", "visual: characters, lines, a block"),
            (":", "a command;  / ?  search"),
            ("Esc", "back to normal mode"),
        ],
    ),
    (
        "Moving",
        &[
            ("h j k l", "left, down, up, right (j and k by screen line)"),
            ("w b e  W B E  ge", "by word"),
            (
                "0 ^ $  gg G  NG",
                "line start, first non-blank, end; first line, last, line N",
            ),
            ("f F t T ; ,", "to a character in the line, and again"),
            ("( ) { }", "by sentence, by paragraph"),
            ("%", "the matching bracket"),
            ("gj gk g0 g$", "by screen line"),
            ("H M L", "top, middle, bottom of the screen"),
            ("Ctrl-D Ctrl-U Ctrl-F Ctrl-B", "half a screen, a screen"),
            (
                "zt zz zb",
                "scroll the cursor's line to the top, middle, bottom",
            ),
            (
                "]f [f  ]k [k  ]h [h",
                "next or last function, class, heading",
            ),
        ],
    ),
    (
        "Changing",
        &[
            (
                "d c y",
                "delete, change, copy (with a motion or text object; dd cc yy for a line)",
            ),
            ("> < gu gU g~", "indent, outdent, lower, upper, swap case"),
            (
                "x X s S r J ~",
                "a character, a line, replace one, join lines, swap case",
            ),
            ("p P", "put after, before"),
            ("u Ctrl-R U", "undo, redo, undo the line"),
            (".", "do the last change again"),
            ("gq gw", "format a paragraph to the width"),
            ("Ctrl-A Ctrl-X", "add to, take from a number"),
        ],
    ),
    (
        "Text objects (after d c y v, i inside, a around)",
        &[
            ("iw aw  is as  ip ap", "word, sentence, paragraph"),
            ("i( i{ i[ i<  i\" i'  it", "brackets, quotes, a tag"),
            (
                "i* il ih ic",
                "Markdown: emphasis, link, heading section, code",
            ),
            ("if ik ia i/", "code: function, class, argument, comment"),
        ],
    ),
    (
        "Search",
        &[
            ("/ ?", "search forward, back"),
            ("n N", "next, previous match"),
            ("* #", "the word under the cursor"),
            (
                ":s/a/b/  :%s/a/b/g",
                "replace, on the line or everywhere (c to ask)",
            ),
            (":noh", "clear the highlighted matches"),
        ],
    ),
    (
        "Marks, registers, macros",
        &[
            (
                "m{a-z}  '{a-z}  `{a-z}",
                "set a mark, go to its line, to it",
            ),
            ("Ctrl-O Ctrl-I", "back and forward through the jumps"),
            ("\"{a-z}", "use a register;  \"+ the clipboard"),
            ("q{a-z}  @{a-z}  @@", "record a macro, play it, again"),
        ],
    ),
    (
        "Commands",
        &[
            (":w  :saveas", "save, save as"),
            (":e  :e {file}", "open (asks), open a file"),
            (
                ":q  :q!  :wq",
                "close, close dropping changes, save and close",
            ),
            (":set ft={language}", "highlight as another language"),
            (
                ":g/a/d  :normal",
                "on every line that matches; keys on lines",
            ),
        ],
    ),
];
