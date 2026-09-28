//! Omavim's Vim engine: keys in, edits and cursor moves out. No GUI.
//!
//! The engine works on any text through [`TextModel`], so it runs the same on
//! the app's buffer and on a plain rope in tests, where its behaviour is checked
//! against real Vim (see PLAN.md, *Testing*).

mod engine;
pub mod key;
mod motion;
pub mod search;
pub mod text;
mod textobj;
pub mod wrap;

pub use engine::{Beep, Register, Vim};
pub use key::Key;

use std::ops::Range;

/// A position in the text, as a char index.
pub type Pos = usize;

/// The text objects (and `]f`-style moves) found from the syntax tree: code's
/// functions, classes, parameters and comments, and Markdown's emphasis,
/// links, heading sections and code blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyntaxObject {
    Function,
    Class,
    Parameter,
    Comment,
    Emphasis,
    Link,
    Heading,
    Code,
}

/// What the engine needs from a text buffer.
pub trait TextModel {
    /// Chars in the whole text.
    fn len_chars(&self) -> usize;

    /// Lines in the text. A trailing line break starts an empty last line.
    fn len_lines(&self) -> usize;

    /// The line a position is on.
    fn char_to_line(&self, pos: Pos) -> usize;

    /// The first position of a line.
    fn line_to_char(&self, line: usize) -> Pos;

    /// The char at a position.
    fn char(&self, pos: Pos) -> char;

    /// A stretch of the text.
    fn slice(&self, range: Range<Pos>) -> String;

    /// Replace a range with new text: every edit goes through here, so the
    /// app can record undo steps and tell tree-sitter what changed.
    fn replace(&mut self, range: Range<Pos>, with: &str);

    /// Screen lines, for `gj`/`gk` and moving by wrapped line: the positions
    /// where each screen line of `line` starts. A line that fits is `[start]`.
    fn screen_line_starts(&self, line: usize) -> Vec<Pos> {
        vec![self.line_to_char(line)]
    }

    /// Every `kind` of syntax object in the text (just the inside of each
    /// when `inner`), as char ranges, from the syntax tree; those in code
    /// embedded in the text too, where it's embedded around `near`. None
    /// without a syntax tree.
    fn syntax_objects(&self, kind: SyntaxObject, inner: bool, near: Pos) -> Vec<Range<Pos>> {
        let _ = (kind, inner, near);
        Vec::new()
    }
}

/// The modes Omavim knows, as in Vim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Normal,
    Insert,
    Replace,
    Visual,
    VisualLine,
    VisualBlock,
    OperatorPending,
    CommandLine,
    /// `:s///c` asking whether to substitute a match.
    Confirm,
}
