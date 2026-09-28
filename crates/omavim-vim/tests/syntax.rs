//! The syntax text objects and moves, over a text model that says where its
//! objects are (the app's comes from tree-sitter).

use omavim_vim::key::parse;
use omavim_vim::{Indenting, Pos, SyntaxObject, TextModel, Vim};
use ropey::Rope;
use std::ops::Range;

struct Doc {
    rope: Rope,
    /// (kind, inner, range).
    objects: Vec<(SyntaxObject, bool, Range<Pos>)>,
    /// Strings and comments.
    regions: Vec<Range<Pos>>,
    indenting: Option<Indenting>,
}

impl TextModel for Doc {
    fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }
    fn len_lines(&self) -> usize {
        TextModel::len_lines(&self.rope)
    }
    fn char_to_line(&self, pos: Pos) -> usize {
        self.rope.char_to_line(pos)
    }
    fn line_to_char(&self, line: usize) -> Pos {
        self.rope.line_to_char(line)
    }
    fn char(&self, pos: Pos) -> char {
        self.rope.char(pos)
    }
    fn slice(&self, range: Range<Pos>) -> String {
        self.rope.slice(range).to_string()
    }
    fn replace(&mut self, range: Range<Pos>, with: &str) {
        self.rope.remove(range.clone());
        self.rope.insert(range.start, with);
    }
    fn syntax_objects(&self, kind: SyntaxObject, inner: bool, _near: Pos) -> Vec<Range<Pos>> {
        self.objects
            .iter()
            .filter(|o| o.0 == kind && o.1 == inner)
            .map(|o| o.2.clone())
            .collect()
    }
    fn smart_indent(&self) -> Option<Indenting> {
        self.indenting.clone()
    }
    fn syntax_region(&self, pos: Pos) -> Option<Range<Pos>> {
        self.regions.iter().find(|r| r.contains(&pos)).cloned()
    }
}

/// The char range of the first `what` in `text`.
fn find(text: &str, what: &str) -> Range<Pos> {
    let b = text.find(what).unwrap();
    let start = text[..b].chars().count();
    start..start + what.chars().count()
}

const CODE: &str = "fn one(a: u32, b: u32) {\n    a + b\n}\n\nfn two() {\n    3\n}\n";

fn code() -> Doc {
    let f1 = find(CODE, "fn one(a: u32, b: u32) {\n    a + b\n}");
    let f2 = find(CODE, "fn two() {\n    3\n}");
    use SyntaxObject::*;
    Doc {
        rope: Rope::from_str(CODE),
        objects: vec![
            (Function, false, f1.start..f1.end + 1),
            (Function, true, find(CODE, "a + b")),
            (Function, false, f2.start..f2.end + 1),
            (
                Function,
                true,
                find(CODE, "    3").start + 4..find(CODE, "    3").end,
            ),
            (Parameter, true, find(CODE, "a: u32")),
            (Parameter, false, find(CODE, "a: u32, ")),
            (Parameter, true, find(CODE, "b: u32")),
            (Parameter, false, find(CODE, ", b: u32")),
        ],
        regions: Vec::new(),
        indenting: None,
    }
}

fn run(doc: &mut Doc, at: Pos, keys: &str) -> Vim {
    let mut vim = Vim::new();
    vim.set_cursor(at);
    for k in parse(keys) {
        let _ = vim.key(doc, k);
    }
    vim
}

#[test]
fn a_function_goes_whole_lines() {
    let mut doc = code();
    run(&mut doc, find(CODE, "a + b").start, "daf");
    assert_eq!(doc.rope.to_string(), "\nfn two() {\n    3\n}\n");
}

#[test]
fn inside_a_function_is_its_body() {
    let mut doc = code();
    run(&mut doc, 0, "cifx<Esc>");
    assert_eq!(doc.rope.to_string(), CODE.replacen("a + b", "x", 1));
}

#[test]
fn with_none_around_it_looks_ahead() {
    let mut doc = code();
    // On the blank line between them: the next function's.
    run(&mut doc, find(CODE, "\n\nfn two").start + 1, "dif");
    assert_eq!(doc.rope.to_string(), CODE.replacen("    3", "    ", 1));
}

#[test]
fn parameters() {
    let mut doc = code();
    run(&mut doc, find(CODE, "b: u32").start, "daa");
    assert_eq!(doc.rope.to_string(), CODE.replacen(", b: u32", "", 1));
    let mut doc = code();
    run(&mut doc, find(CODE, "a: u32").start + 3, "cianame<Esc>");
    assert_eq!(doc.rope.to_string(), CODE.replacen("a: u32", "name", 1));
}

#[test]
fn jumps_to_the_next_and_last_function() {
    let mut doc = code();
    let vim = run(&mut doc, 3, "]f");
    assert_eq!(vim.cursor(), find(CODE, "fn two").start);
    let vim = run(&mut doc, find(CODE, "    3").end - 1, "[f");
    assert_eq!(vim.cursor(), find(CODE, "fn two").start);
    let vim = run(&mut doc, find(CODE, "    3").end - 1, "2[f");
    assert_eq!(vim.cursor(), 0);
    // And back where it was with ``.
    let vim = run(&mut doc, 3, "]f``");
    assert_eq!(vim.cursor(), 3);
}

#[test]
fn visual_grows_to_the_next_one_out() {
    let text = "a *b **c** d* e\n";
    let outer = find(text, "*b **c** d*");
    let inner = find(text, "**c**");
    let mut doc = Doc {
        rope: Rope::from_str(text),
        objects: vec![
            (SyntaxObject::Emphasis, false, outer.clone()),
            (SyntaxObject::Emphasis, false, inner.clone()),
        ],
        regions: Vec::new(),
        indenting: None,
    };
    run(&mut doc, find(text, "c").start, "va*a*d");
    assert_eq!(doc.rope.to_string(), "a  e\n");
}

#[test]
fn percent_skips_brackets_in_strings_and_comments() {
    let text = "f(\")\", x) // (\ng(\"(\", \")\")\n";
    let last = {
        let b = text.rfind("\")\"").unwrap();
        let start = text[..b].chars().count();
        start..start + 3
    };
    let doc = || Doc {
        rope: Rope::from_str(text),
        objects: Vec::new(),
        regions: vec![
            find(text, "\")\""),
            find(text, "// (\n"),
            find(text, "\"(\""),
            last.clone(),
        ],
        indenting: None,
    };
    let vim = run(&mut doc(), 1, "%");
    assert_eq!(vim.cursor(), find(text, ", x)").end - 1);
    // In a string, only the string's own: there's none, so it stays.
    let at = find(text, "\"(\"").start + 1;
    let vim = run(&mut doc(), at, "%");
    assert_eq!(vim.cursor(), at);
    let g = find(text, "g(").start + 1;
    let vim = run(&mut doc(), g, "%");
    assert_eq!(vim.cursor(), text.chars().count() - 2);
}

fn indenting(text: &str, colon: bool) -> Doc {
    Doc {
        rope: Rope::from_str(text),
        objects: Vec::new(),
        regions: Vec::new(),
        indenting: Some(Indenting {
            unit: "    ".into(),
            colon,
        }),
    }
}

#[test]
fn a_new_line_in_a_block_goes_in() {
    let mut doc = indenting("fn a() {", false);
    run(&mut doc, 0, "A<CR>x<Esc>");
    assert_eq!(doc.rope.to_string(), "fn a() {\n    x");
    let mut doc = indenting("  f(", false);
    run(&mut doc, 0, "ox<Esc>");
    assert_eq!(doc.rope.to_string(), "  f(\n      x");
    let mut doc = indenting("if a:", true);
    run(&mut doc, 0, "ox<Esc>");
    assert_eq!(doc.rope.to_string(), "if a:\n    x");
    // Not a `:` where it doesn't open a block.
    let mut doc = indenting("a:", false);
    run(&mut doc, 0, "ox<Esc>");
    assert_eq!(doc.rope.to_string(), "a:\nx");
}

#[test]
fn enter_between_brackets_puts_the_close_on_its_own_line() {
    let mut doc = indenting("  x = {}", false);
    run(&mut doc, 0, "f}i<CR>y<Esc>");
    assert_eq!(doc.rope.to_string(), "  x = {\n      y\n  }");
}

#[test]
fn above_a_close_goes_in() {
    let mut doc = indenting("f {\n}", false);
    run(&mut doc, 4, "Ox<Esc>");
    assert_eq!(doc.rope.to_string(), "f {\n    x\n}");
}

#[test]
fn a_close_typed_first_goes_back_out() {
    let mut doc = indenting("  f {\n      x", false);
    run(&mut doc, 8, "o}<Esc>");
    assert_eq!(doc.rope.to_string(), "  f {\n      x\n  }");
}

#[test]
fn a_bracket_in_a_string_opens_nothing() {
    let mut doc = indenting("s = \"{\"", false);
    doc.regions.push(4..7);
    run(&mut doc, 0, "A<CR>x<Esc>");
    assert_eq!(doc.rope.to_string(), "s = \"{\"\nx");
    let mut doc = indenting("s = \"{", false);
    doc.regions.push(4..6);
    run(&mut doc, 0, "ox<Esc>");
    assert_eq!(doc.rope.to_string(), "s = \"{\nx");
}
