//! The open document: its text, where it lives, and whether it has unsaved
//! changes. The cursor and all editing belong to the Vim engine.

use ropey::Rope;
use std::path::PathBuf;

#[derive(Debug)]
pub struct Document {
    /// Lines joined by '\n', without the file's final line break (as Vim
    /// holds a buffer: a final "\n" isn't an extra empty line).
    pub text: Rope,
    pub path: Option<PathBuf>,
    /// Changed since it was opened or last saved.
    pub dirty: bool,
    /// The file ended with a line break, so saving writes one back.
    pub final_newline: bool,
}

impl Default for Document {
    fn default() -> Self {
        // New files end with a line break, as Vim writes them.
        Self {
            text: Rope::new(),
            path: None,
            dirty: false,
            final_newline: true,
        }
    }
}

impl Document {
    pub fn open(path: PathBuf, contents: &str) -> Self {
        let (body, final_newline) = match contents.strip_suffix('\n') {
            Some(body) => (body, true),
            None => (contents, contents.is_empty()),
        };
        Self {
            text: Rope::from_str(body),
            path: Some(path),
            dirty: false,
            final_newline,
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
