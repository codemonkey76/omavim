//! The open document: its text, where it lives, and whether it has unsaved
//! changes. The cursor and all editing belong to the Vim engine.

use omavim_syntax::{InputEdit, Lang, Point, Syntax};
use ropey::Rope;
use std::ops::Range;
use std::path::PathBuf;

pub struct Document {
    /// Lines joined by '\n', without the file's final line break (as Vim
    /// holds a buffer: a final "\n" isn't an extra empty line).
    pub text: Rope,
    pub path: Option<PathBuf>,
    /// Changed since it was opened or last saved.
    pub dirty: bool,
    /// The file ended with a line break, so saving writes one back.
    pub final_newline: bool,
    /// Its syntax tree, for a language Omavim knows.
    pub syntax: Option<Syntax>,
}

impl Default for Document {
    fn default() -> Self {
        // New files end with a line break, as Vim writes them.
        Self {
            text: Rope::new(),
            path: None,
            dirty: false,
            final_newline: true,
            syntax: None,
        }
    }
}

impl Document {
    pub fn open(path: PathBuf, contents: &str) -> Self {
        let (body, final_newline) = match contents.strip_suffix('\n') {
            Some(body) => (body, true),
            None => (contents, contents.is_empty()),
        };
        let mut doc = Self {
            text: Rope::from_str(body),
            path: Some(path),
            dirty: false,
            final_newline,
            syntax: None,
        };
        doc.detect_language();
        doc
    }

    /// The language by the file's name (again, after `:saveas`).
    pub fn detect_language(&mut self) {
        let lang = self.path.as_deref().and_then(Lang::from_path);
        self.set_language(lang);
    }

    /// Highlight as this language (`None`: plain text).
    pub fn set_language(&mut self, lang: Option<Lang>) {
        if self.syntax.as_ref().map(Syntax::lang) == lang {
            return;
        }
        self.syntax = lang.map(|l| {
            let mut s = Syntax::new(l);
            s.parse(&self.text);
            s
        });
    }

    /// The text was edited: bring the syntax tree up to date.
    pub fn edited(&mut self, edits: &[InputEdit]) {
        if let Some(s) = &mut self.syntax
            && !edits.is_empty()
        {
            for e in edits {
                s.edit(e);
            }
            s.parse(&self.text);
        }
    }

    /// What to write to the file.
    pub fn contents(&self) -> String {
        let mut s = self.text.to_string();
        if self.final_newline {
            s.push('\n');
        }
        s
    }

    /// The name to show: the file's name, or "Untitled".
    pub fn name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".into())
    }
}

/// The text, for the Vim engine to edit, noting each edit for tree-sitter.
pub struct Recorder<'a> {
    pub text: &'a mut Rope,
    pub edits: Vec<InputEdit>,
}

impl<'a> Recorder<'a> {
    pub fn new(text: &'a mut Rope) -> Self {
        Self {
            text,
            edits: Vec::new(),
        }
    }
}

/// Where a byte is, as tree-sitter counts: its line, and bytes into it.
fn point(text: &Rope, byte: usize) -> Point {
    let line = text.byte_to_line(byte);
    Point::new(line, byte - text.line_to_byte(line))
}

impl omavim_vim::TextModel for Recorder<'_> {
    fn len_chars(&self) -> usize {
        self.text.len_chars()
    }
    fn len_lines(&self) -> usize {
        self.text.len_lines()
    }
    fn char_to_line(&self, pos: usize) -> usize {
        self.text.char_to_line(pos)
    }
    fn line_to_char(&self, line: usize) -> usize {
        self.text.line_to_char(line)
    }
    fn char(&self, pos: usize) -> char {
        self.text.char(pos)
    }
    fn slice(&self, range: Range<usize>) -> String {
        self.text.slice(range).to_string()
    }
    fn replace(&mut self, range: Range<usize>, with: &str) {
        let start_byte = self.text.char_to_byte(range.start);
        let old_end_byte = self.text.char_to_byte(range.end);
        let start_position = point(self.text, start_byte);
        let old_end_position = point(self.text, old_end_byte);
        omavim_vim::TextModel::replace(&mut *self.text, range, with);
        let new_end_byte = start_byte + with.len();
        self.edits.push(InputEdit {
            start_byte,
            old_end_byte,
            new_end_byte,
            start_position,
            old_end_position,
            new_end_position: point(self.text, new_end_byte),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_final_line_break_is_kept_but_isnt_an_empty_line() {
        let d = Document::open("x.md".into(), "one\ntwo\n");
        assert_eq!(d.text.len_lines(), 2);
        assert_eq!(d.contents(), "one\ntwo\n");
        let d = Document::open("x.md".into(), "no break");
        assert_eq!(d.contents(), "no break");
    }

    #[test]
    fn names_an_unsaved_document_untitled() {
        assert_eq!(Document::default().name(), "Untitled");
        assert_eq!(
            Document::open("/tmp/notes.md".into(), "").name(),
            "notes.md"
        );
    }
}
