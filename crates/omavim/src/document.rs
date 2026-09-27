//! The open document: its text, where it lives, whether it has unsaved
//! changes, and the cursor.
//!
//! Editing here is plain typing for now (milestone 1). The Vim engine
//! (`omavim-vim`, milestone 2) takes over the keys and edits this same text.

use ropey::Rope;
use std::path::PathBuf;

#[derive(Debug, Default)]
pub struct Document {
    pub text: Rope,
    pub path: Option<PathBuf>,
    /// Changed since it was opened or last saved.
    pub dirty: bool,
    /// The cursor, as a char index into `text`.
    pub cursor: usize,
    /// The column `Up`/`Down` try to keep, so moving through a short line
    /// doesn't lose it.
    goal_column: Option<usize>,
}

impl Document {
    pub fn open(path: PathBuf, contents: &str) -> Self {
        Self {
            text: Rope::from_str(contents),
            path: Some(path),
            ..Self::default()
        }
    }

    /// The name to show: the file's name, or "Untitled".
    pub fn name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".into())
    }

    /// The cursor's line and column, both from 0 (column in chars).
    pub fn line_col(&self) -> (usize, usize) {
        let line = self.text.char_to_line(self.cursor);
        (line, self.cursor - self.text.line_to_char(line))
    }

    /// Chars in a line, not counting its line break.
    fn line_len(&self, line: usize) -> usize {
        let slice = self.text.line(line);
        let mut len = slice.len_chars();
        if len > 0 && slice.char(len - 1) == '\n' {
            len -= 1;
            if len > 0 && slice.char(len - 1) == '\r' {
                len -= 1;
            }
        }
        len
    }

    pub fn insert(&mut self, s: &str) {
        self.text.insert(self.cursor, s);
        self.cursor += s.chars().count();
        self.goal_column = None;
        self.dirty = true;
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.text.remove(self.cursor - 1..self.cursor);
            self.cursor -= 1;
            self.goal_column = None;
            self.dirty = true;
        }
    }

    pub fn delete(&mut self) {
        if self.cursor < self.text.len_chars() {
            self.text.remove(self.cursor..self.cursor + 1);
            self.goal_column = None;
            self.dirty = true;
        }
    }

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
        self.goal_column = None;
    }

    pub fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.text.len_chars());
        self.goal_column = None;
    }

    pub fn home(&mut self) {
        let (line, _) = self.line_col();
        self.cursor = self.text.line_to_char(line);
        self.goal_column = None;
    }

    pub fn end(&mut self) {
        let (line, _) = self.line_col();
        self.cursor = self.text.line_to_char(line) + self.line_len(line);
        self.goal_column = None;
    }

    /// Move `lines` up (negative) or down, keeping the goal column.
    pub fn vertical(&mut self, lines: isize) {
        let (line, col) = self.line_col();
        let goal = *self.goal_column.get_or_insert(col);
        let last = self.text.len_lines().saturating_sub(1);
        let target = (line as isize + lines).clamp(0, last as isize) as usize;
        self.cursor = self.text.line_to_char(target) + goal.min(self.line_len(target));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_and_moving_keep_the_goal_column() {
        let mut d = Document::open("x.md".into(), "hello world\nhi\nthird line\n");
        d.vertical(0);
        for _ in 0..8 {
            d.right();
        }
        assert_eq!(d.line_col(), (0, 8));
        d.vertical(1); // "hi" is shorter: lands at its end
        assert_eq!(d.line_col(), (1, 2));
        d.vertical(1); // back to column 8 on a long enough line
        assert_eq!(d.line_col(), (2, 8));
        d.insert("X");
        assert_eq!(d.text.line(2).to_string(), "third liXne\n");
        assert!(d.dirty);
    }

    #[test]
    fn backspace_joins_lines() {
        let mut d = Document::open("x.md".into(), "a\nb");
        d.vertical(1);
        d.home();
        d.backspace();
        assert_eq!(d.text.to_string(), "ab");
        assert_eq!(d.line_col(), (0, 1));
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
