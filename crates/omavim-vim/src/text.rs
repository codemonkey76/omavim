//! Text helpers over a [`TextModel`]: lines and columns, and Vim's character
//! classes. Text is lines joined by '\n' with no line break after the last.

use crate::{Pos, TextModel};
use ropey::Rope;
use std::ops::Range;

impl TextModel for Rope {
    fn len_chars(&self) -> usize {
        Rope::len_chars(self)
    }
    fn len_lines(&self) -> usize {
        Rope::len_lines(self)
    }
    fn char_to_line(&self, pos: Pos) -> usize {
        Rope::char_to_line(self, pos)
    }
    fn line_to_char(&self, line: usize) -> Pos {
        Rope::line_to_char(self, line)
    }
    fn char(&self, pos: Pos) -> char {
        Rope::char(self, pos)
    }
    fn slice(&self, range: Range<Pos>) -> String {
        Rope::slice(self, range).to_string()
    }
    fn replace(&mut self, range: Range<Pos>, with: &str) {
        if !range.is_empty() {
            self.remove(range.clone());
        }
        if !with.is_empty() {
            self.insert(range.start, with);
        }
    }
}

/// Chars in a line, not counting its line break.
pub fn line_len(t: &dyn TextModel, line: usize) -> usize {
    let start = t.line_to_char(line);
    let end = if line + 1 < t.len_lines() {
        t.line_to_char(line + 1) - 1
    } else {
        t.len_chars()
    };
    end - start
}

pub fn last_line(t: &dyn TextModel) -> usize {
    t.len_lines() - 1
}

pub fn pos(t: &dyn TextModel, line: usize, col: usize) -> Pos {
    t.line_to_char(line) + col
}

pub fn line_col(t: &dyn TextModel, p: Pos) -> (usize, usize) {
    let line = t.char_to_line(p);
    (line, p - t.line_to_char(line))
}

/// The char at a line and column, or None at the end of the line (Vim's NUL).
pub fn char_at(t: &dyn TextModel, line: usize, col: usize) -> Option<char> {
    (col < line_len(t, line)).then(|| t.char(pos(t, line, col)))
}

pub fn line_text(t: &dyn TextModel, line: usize) -> String {
    let start = t.line_to_char(line);
    t.slice(start..start + line_len(t, line))
}

pub fn is_blank(c: char) -> bool {
    c == ' ' || c == '\t'
}

/// Column of the first non-blank char (the line's length if it's all blank).
pub fn first_non_blank(t: &dyn TextModel, line: usize) -> usize {
    let len = line_len(t, line);
    (0..len)
        .find(|&c| !is_blank(t.char(pos(t, line, c))))
        .unwrap_or(len)
}

/// The line's leading whitespace.
pub fn indent(t: &dyn TextModel, line: usize) -> String {
    let start = t.line_to_char(line);
    t.slice(start..start + first_non_blank(t, line))
}

/// Screen columns a char takes after `vcol`: tabs to the next tab stop,
/// wide (East Asian) chars 2, others 1.
pub fn char_width(c: char, vcol: usize, tabstop: usize) -> usize {
    if c == '\t' {
        return tabstop - vcol % tabstop;
    }
    let n = c as u32;
    let wide = matches!(n, 0x1100..=0x115f | 0x2e80..=0x303e | 0x3041..=0x33ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff
        | 0xa000..=0xa4cf | 0xac00..=0xd7a3 | 0xf900..=0xfaff | 0xfe30..=0xfe4f | 0xff00..=0xff60 | 0xffe0..=0xffe6
        | 0x1f300..=0x1f64f | 0x1f900..=0x1f9ff | 0x20000..=0x3fffd);
    if wide { 2 } else { 1 }
}

/// The screen column where the char at `col` starts (Vim's virtual column).
pub fn vcol(t: &dyn TextModel, line: usize, col: usize, tabstop: usize) -> usize {
    let start = t.line_to_char(line);
    (0..col.min(line_len(t, line))).fold(0, |v, i| v + char_width(t.char(start + i), v, tabstop))
}

/// The char at screen column `vcol`: the one whose cells cover it, or the
/// line's length when the line is shorter.
pub fn col_at_vcol(t: &dyn TextModel, line: usize, vcol: usize, tabstop: usize) -> usize {
    let start = t.line_to_char(line);
    let len = line_len(t, line);
    let mut v = 0;
    for i in 0..len {
        let w = char_width(t.char(start + i), v, tabstop);
        if v + w > vcol {
            return i;
        }
        v += w;
    }
    len
}

/// Vim's 'iskeyword' default: letters, digits, '_' and chars 192-255.
pub fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || (c as u32 >= 192 && c as u32 <= 255)
}

/// Vim's character class (`cls()`): 0 blank or end of line, 1 punctuation,
/// 2 word chars, and Vim's `utf_class` values for other scripts, so a word
/// ends where the script changes. With `big`, every non-blank is class 1.
pub fn class(c: Option<char>, big: bool) -> u32 {
    let Some(c) = c else { return 0 };
    let n = c as u32;
    let class = if n < 0x100 {
        if c == ' ' || c == '\t' || c == '\u{a0}' {
            0
        } else if is_word_char(c) {
            2
        } else {
            1
        }
    } else {
        match n {
            0x2000..=0x200b | 0x2028..=0x2029 | 0x205f | 0x3000 => 0,
            0x037e
            | 0x0387
            | 0x055a..=0x055f
            | 0x0589
            | 0x05be
            | 0x05c0
            | 0x05c3
            | 0x05f3..=0x05f4 => 1,
            0x200c..=0x2027
            | 0x202a..=0x205e
            | 0x2060..=0x27ff
            | 0x2e00..=0x2e7f
            | 0x3001..=0x3020
            | 0x30fb => 1,
            0x3040..=0x309f => 0x3040,
            0x30a0..=0x30ff => 0x30a0,
            0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff | 0x20000..=0x2fa1f => 0x4e00,
            0xac00..=0xd7a3 => 0xac00,
            0xff00..=0xff0f | 0xff1a..=0xff20 | 0xff3b..=0xff40 | 0xff5b..=0xff65 => 1,
            0x1f000..=0x1faff => 3,
            _ => 2,
        }
    };
    if class != 0 && big { 1 } else { class }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_and_columns() {
        let r = Rope::from_str("ab\n\ncde");
        assert_eq!(line_len(&r, 0), 2);
        assert_eq!(line_len(&r, 1), 0);
        assert_eq!(line_len(&r, 2), 3);
        assert_eq!(char_at(&r, 0, 2), None);
        assert_eq!(line_col(&r, pos(&r, 2, 1)), (2, 1));
        assert_eq!(first_non_blank(&Rope::from_str("  \tx"), 0), 3);
    }

    #[test]
    fn classes_split_words_as_vim_does() {
        assert_eq!(class(Some('a'), false), 2);
        assert_eq!(class(Some('é'), false), 2);
        assert_eq!(class(Some('-'), false), 1);
        assert_eq!(class(Some('—'), false), 1);
        assert_eq!(class(Some('日'), false), 0x4e00);
        assert_eq!(class(Some('日'), true), 1);
        assert_eq!(class(None, false), 0);
    }
}
