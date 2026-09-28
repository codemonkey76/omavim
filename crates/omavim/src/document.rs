//! The open document: its text, where it lives, and whether it has unsaved
//! changes. The cursor and all editing belong to the Vim engine.

use omavim_syntax::{InputEdit, Lang, Point, Syntax};
use omavim_vim::{Indenting, SyntaxObject};
use ropey::Rope;
use std::cell::RefCell;
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

    /// Its language's name, as 'filetype' has it ("" for plain text).
    pub fn filetype(&self) -> &'static str {
        self.syntax.as_ref().map_or("", |s| s.lang().name())
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

/// The text, for the Vim engine to edit: each edit goes to the syntax tree
/// too, which is parsed again here when a text object needs it (else on
/// another thread, after the keys: see `App::parse_later`).
pub struct Recorder<'a> {
    text: &'a mut Rope,
    syntax: RefCell<Option<&'a mut Syntax>>,
}

impl<'a> Recorder<'a> {
    pub fn new(doc: &'a mut Document) -> Self {
        Self {
            text: &mut doc.text,
            syntax: RefCell::new(doc.syntax.as_mut()),
        }
    }

    /// Bring the syntax tree up to date with the edits, now: for what
    /// needs it exact (a text object, `%`).
    fn parse(&self) {
        if let Some(s) = self.syntax.borrow_mut().as_mut()
            && s.stale()
        {
            s.parse(self.text);
        }
    }
}

/// How far code in the text is indented a level: a tab, or the step
/// between lines' indents seen most, else the language's usual.
fn indent_unit(text: &Rope, lang: Lang) -> String {
    let mut steps = [0usize; 9];
    let mut tabs = 0;
    let mut last = 0;
    for line in text.lines().take(2000) {
        let spaces = line.chars().take_while(|&c| c == ' ').count();
        if line.chars().next() == Some('\t') {
            tabs += 1;
            continue;
        }
        if line.chars().all(char::is_whitespace) {
            continue;
        }
        if (2..=8).contains(&spaces.saturating_sub(last)) {
            steps[spaces - last] += 1;
        }
        last = spaces;
    }
    let (step, seen) = (2..=8)
        .map(|n| (n, steps[n]))
        .max_by_key(|&(n, c)| (c, n == 4))
        .unwrap();
    if tabs > seen {
        return "\t".into();
    }
    if seen > 0 {
        return " ".repeat(step);
    }
    match lang {
        Lang::Go => "\t".into(),
        Lang::Yaml
        | Lang::Json
        | Lang::Lua
        | Lang::Html
        | Lang::Css
        | Lang::JavaScript
        | Lang::TypeScript
        | Lang::Tsx => "  ".into(),
        _ => "    ".into(),
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
        let edit = InputEdit {
            start_byte,
            old_end_byte,
            new_end_byte,
            start_position,
            old_end_position,
            new_end_position: point(self.text, new_end_byte),
        };
        if let Some(s) = self.syntax.get_mut().as_mut() {
            s.edit(&edit);
        }
    }

    fn smart_indent(&self) -> Option<Indenting> {
        let lang = self.syntax.borrow().as_ref()?.lang();
        if matches!(lang, Lang::Markdown | Lang::MarkdownInline) {
            return None;
        }
        Some(Indenting {
            unit: indent_unit(self.text, lang),
            colon: matches!(lang, Lang::Python | Lang::Yaml),
        })
    }

    fn syntax_region(&self, pos: usize) -> Option<Range<usize>> {
        self.parse();
        let syntax = self.syntax.borrow();
        let s = syntax.as_ref()?;
        let r = s.region(self.text.char_to_byte(pos.min(self.text.len_chars())))?;
        let len = self.text.len_bytes();
        Some(self.text.byte_to_char(r.start.min(len))..self.text.byte_to_char(r.end.min(len)))
    }

    fn syntax_objects(&self, kind: SyntaxObject, inner: bool, near: usize) -> Vec<Range<usize>> {
        self.parse();
        let syntax = self.syntax.borrow();
        let Some(s) = syntax.as_ref() else {
            return Vec::new();
        };
        let kind = match kind {
            SyntaxObject::Function => "function",
            SyntaxObject::Class => "class",
            SyntaxObject::Parameter => "parameter",
            SyntaxObject::Comment => "comment",
            SyntaxObject::Emphasis => "emphasis",
            SyntaxObject::Link => "link",
            SyntaxObject::Heading => "heading",
            SyntaxObject::Code => "code",
        };
        let near = self.text.char_to_byte(near.min(self.text.len_chars()));
        let len = self.text.len_bytes();
        s.objects(self.text, kind, inner, near)
            .into_iter()
            .map(|r| {
                self.text.byte_to_char(r.start.min(len))..self.text.byte_to_char(r.end.min(len))
            })
            .collect()
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
    fn finds_how_far_a_file_indents() {
        let unit = |text: &str, lang| indent_unit(&Rope::from_str(text), lang);
        assert_eq!(unit("a {\n  b {\n    c\n  }\n}\n", Lang::Rust), "  ");
        assert_eq!(unit("a {\n\tb\n}\n", Lang::Rust), "\t");
        assert_eq!(unit("a\n", Lang::Rust), "    ");
        assert_eq!(unit("a\n", Lang::Yaml), "  ");
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
