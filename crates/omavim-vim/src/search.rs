//! Search: Vim's patterns translated for the regex crate, and finding the
//! next or previous match the way Vim's searchit() does.

use regex::{Regex, RegexBuilder};

use crate::text::line_len;
use crate::{Pos, TextModel};

/// A compiled Vim pattern.
#[derive(Debug, Clone)]
pub struct Pattern {
    re: Regex,
    /// The match is the `m` group (the pattern used `\zs` or `\ze`).
    group: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Magic {
    Very,
    Normal,
    No,
    VeryNo,
}

/// Chars with a meaning without a backslash, in each of Vim's modes.
fn special(m: Magic, c: char) -> bool {
    match m {
        Magic::Very => "^$.*[~(){}|+?=@%<>&".contains(c),
        Magic::Normal => "^$.*[~".contains(c),
        Magic::No => "^$".contains(c),
        Magic::VeryNo => false,
    }
}

/// A Vim character class (`\s`, `\d`, ...) as a regex class; `nl`: with a
/// line break too (`\_s`). The negated ones never match a line break
/// unless asked, as in Vim.
fn class(c: char, nl: bool) -> Option<String> {
    let (set, neg) = match c {
        's' => (" \\t", false),
        'S' => (" \\t", true),
        'd' => ("0-9", false),
        'D' => ("0-9", true),
        'w' => ("0-9A-Za-z_", false),
        'W' => ("0-9A-Za-z_", true),
        'a' => ("A-Za-z", false),
        'A' => ("A-Za-z", true),
        'l' => ("a-z", false),
        'L' => ("a-z", true),
        'u' => ("A-Z", false),
        'U' => ("A-Z", true),
        'x' => ("0-9A-Fa-f", false),
        'X' => ("0-9A-Fa-f", true),
        'o' => ("0-7", false),
        'O' => ("0-7", true),
        'h' => ("A-Za-z_", false),
        'H' => ("A-Za-z_", true),
        // Keyword and identifier chars: letters, digits, `_`, and (as Vim
        // counts them) chars past Latin-1.
        'k' | 'i' => ("0-9A-Za-z_\\u{C0}-\\u{10FFFF}", false),
        'K' | 'I' => ("A-Za-z_\\u{C0}-\\u{10FFFF}", false),
        'f' => ("0-9A-Za-z_/.\\-+,#$%~=\\u{C0}-\\u{10FFFF}", false),
        'F' => ("A-Za-z_/.\\-+,#$%~=\\u{C0}-\\u{10FFFF}", false),
        'p' => ("[:print:]\\u{A0}-\\u{10FFFF}", false),
        'P' => ("[:print:]\\u{A0}-\\u{10FFFF}", false),
        _ => return None,
    };
    Some(match (neg, nl) {
        (false, false) => format!("[{set}]"),
        (false, true) => format!("[{set}\\n]"),
        (true, false) => format!("[^{set}\\n]"),
        (true, true) => format!("[^{set}]"),
    })
}

fn lit(c: char) -> String {
    regex::escape(&c.to_string())
}

/// A char inside a regex class.
fn class_lit(c: char) -> String {
    if "[]\\^-&~".contains(c) {
        format!("\\{c}")
    } else {
        c.to_string()
    }
}

/// `[...]` starting after the `[`: the regex class and the chars it used,
/// or None if there's no closing `]` (then the `[` is literal).
fn bracket(p: &[char], nl: bool) -> Option<(String, usize)> {
    let mut i = 0;
    let neg = p.first() == Some(&'^');
    if neg {
        i += 1;
    }
    let mut out = String::from(if neg { "[^" } else { "[" });
    let mut first = true;
    let mut prev_char = false;
    loop {
        let c = *p.get(i)?;
        if c == ']' && !first {
            break;
        }
        first = false;
        if c == '[' && p.get(i + 1) == Some(&':') {
            // [:alpha:] and friends, as the regex crate has them.
            let rest: String = p[i..].iter().collect();
            if let Some(end) = rest.find(":]") {
                out.push_str(&rest[..end + 2]);
                i += rest[..end + 2].chars().count();
                prev_char = false;
                continue;
            }
        }
        if c == '-' && prev_char && p.get(i + 1).is_some_and(|&n| n != ']') {
            out.push('-');
            i += 1;
            prev_char = false;
            continue;
        }
        if c == '\\' {
            let n = *p.get(i + 1)?;
            let v = match n {
                'e' => Some('\x1b'),
                't' => Some('\t'),
                'r' => Some('\r'),
                'n' => Some('\n'),
                'b' => Some('\x08'),
                '\\' | ']' | '^' | '-' => Some(n),
                _ => None,
            };
            match v {
                Some(v) => {
                    out.push_str(&class_lit(v));
                    i += 2;
                }
                None => {
                    // Any other backslash is itself.
                    out.push_str("\\\\");
                    i += 1;
                }
            }
            prev_char = true;
            continue;
        }
        out.push_str(&class_lit(c));
        prev_char = true;
        i += 1;
    }
    // Vim's [^...] doesn't match a line break; \_[...] adds it.
    if neg != nl {
        out.push_str("\\n");
    }
    out.push(']');
    Some((out, i + 1))
}

/// `\{n,m}` starting after the `{`: the quantifier and the chars used.
fn brace(p: &[char]) -> Result<(String, usize), String> {
    let mut i = 0;
    let mut body = String::new();
    loop {
        match p.get(i) {
            None => return Err("E554: Syntax error in \\{...}".into()),
            Some('}') => {
                i += 1;
                break;
            }
            Some('\\') if p.get(i + 1) == Some(&'}') => {
                i += 2;
                break;
            }
            Some(&c) => body.push(c),
        }
        i += 1;
    }
    let lazy = body.starts_with('-');
    let body = body.trim_start_matches('-');
    let q = if body.is_empty() {
        "*".to_string()
    } else if let Some((a, b)) = body.split_once(',') {
        let a = if a.is_empty() { "0" } else { a };
        format!("{{{a},{b}}}")
    } else {
        format!("{{{body}}}")
    };
    if !body.chars().all(|c| c.is_ascii_digit() || c == ',') {
        return Err("E554: Syntax error in \\{...}".into());
    }
    Ok((if lazy { format!("{q}?") } else { q }, i))
}

/// Compile a Vim pattern. `ignorecase`/`smartcase` as Vim's options; `\c`
/// and `\C` in the pattern win.
pub fn compile(pat: &str, ignorecase: bool, smartcase: bool) -> Result<Pattern, String> {
    let p: Vec<char> = pat.chars().collect();
    let mut out = String::new();
    let mut mode = Magic::Normal;
    let mut case: Option<bool> = None;
    let mut upper = false;
    // Where `\zs` and `\ze` fell in `out`.
    let (mut zs, mut ze) = (None, None);
    // `out` is at a point where `^` is start-of-line (and `*` literal).
    let mut at_start = true;
    // Capturing groups so far.
    let mut groups = 0;
    let mut i = 0;
    let bad = |what: &str| Err(format!("E867: Unsupported in omavim's search: {what}"));
    while i < p.len() {
        let c = p[i];
        let escaped = c == '\\';
        let (ch, used) = if escaped {
            match p.get(i + 1) {
                Some(&n) => (n, 2),
                None => ('\\', 1),
            }
        } else {
            (c, 1)
        };
        let start_before = at_start;
        at_start = false;
        if escaped && used == 2 {
            // Backslash-letter items mean the same in every mode.
            match ch {
                'v' => mode = Magic::Very,
                'm' => mode = Magic::Normal,
                'M' => mode = Magic::No,
                'V' => mode = Magic::VeryNo,
                'c' => case = Some(true),
                'C' => case = Some(false),
                'n' => out.push_str("\\n"),
                't' => out.push_str("\\t"),
                'e' => out.push_str("\\x1b"),
                'r' => out.push_str("\\r"),
                'z' => match p.get(i + 2) {
                    Some('s') => {
                        zs = Some(out.len());
                        i += 1;
                    }
                    Some('e') => {
                        ze = Some(out.len());
                        i += 1;
                    }
                    _ => return bad("\\z"),
                },
                '_' => {
                    let n = p.get(i + 2).copied().unwrap_or(' ');
                    i += 1;
                    match n {
                        '.' => out.push_str("(?s:.)"),
                        '^' => out.push('^'),
                        '$' => out.push('$'),
                        '[' => match bracket(&p[i + 2..], true) {
                            Some((cls, n)) => {
                                out.push_str(&cls);
                                i += n;
                            }
                            None => return bad("\\_["),
                        },
                        n => match class(n, true) {
                            Some(cls) => out.push_str(&cls),
                            None => return bad("\\_"),
                        },
                    }
                }
                ch if ch.is_ascii_alphabetic() => match class(ch, false) {
                    Some(cls) => out.push_str(&cls),
                    None => return bad(&format!("\\{ch}")),
                },
                _ => {}
            }
            if ch.is_ascii_alphabetic() || ch == '_' {
                if matches!(ch, 'v' | 'm' | 'M' | 'V' | 'c' | 'C' | 'z') {
                    at_start = start_before;
                }
                i += used;
                continue;
            }
        }
        // A punctuation char: special if escaped XOR special in this mode.
        let is_special = if escaped && used == 2 {
            !special(mode, ch)
        } else {
            special(mode, ch)
        };
        if !is_special {
            if ch.is_uppercase() {
                upper = true;
            }
            out.push_str(&lit(ch));
            i += used;
            continue;
        }
        match ch {
            '^' => {
                if start_before || mode == Magic::Very {
                    out.push('^');
                    at_start = true;
                } else {
                    out.push_str("\\^");
                }
            }
            '$' => {
                // An anchor at the end, or before `\|`, `\)` or `\n`.
                let rest: String = p[i + used..].iter().collect();
                let end = mode == Magic::Very
                    || rest.is_empty()
                    || ["\\|", "\\)", "\\n"].iter().any(|s| rest.starts_with(s))
                    || (mode == Magic::Very && (rest.starts_with('|') || rest.starts_with(')')));
                out.push_str(if end { "$" } else { "\\$" });
            }
            '.' => out.push('.'),
            '*' => {
                if start_before {
                    out.push_str("\\*");
                } else {
                    out.push('*');
                }
            }
            '[' => match bracket(&p[i + used..], false) {
                Some((cls, n)) => {
                    out.push_str(&cls);
                    i += n;
                }
                None => out.push_str("\\["),
            },
            '~' => out.push('~'),
            '(' => {
                // Named, so \zs's group can't shift the numbers `\1` uses.
                groups += 1;
                out.push_str(&format!("(?P<g{groups}>"));
                at_start = true;
            }
            ')' => out.push(')'),
            '|' => {
                out.push('|');
                at_start = true;
            }
            '+' => out.push('+'),
            '?' | '=' => out.push('?'),
            '{' => {
                let (q, n) = brace(&p[i + used..])?;
                out.push_str(&q);
                i += n;
            }
            '<' => out.push_str("\\b{start}"),
            '>' => out.push_str("\\b{end}"),
            '%' => match p.get(i + used) {
                Some('(') => {
                    out.push_str("(?:");
                    at_start = true;
                    i += 1;
                }
                Some('^') => {
                    out.push_str("\\A");
                    i += 1;
                }
                Some('$') => {
                    out.push_str("\\z");
                    i += 1;
                }
                _ => return bad("\\%"),
            },
            '@' => return bad("\\@"),
            '&' => return bad("\\&"),
            _ => out.push_str(&lit(ch)),
        }
        i += used;
    }
    let group = zs.is_some() || ze.is_some();
    if group {
        let (s, e) = (zs.unwrap_or(0), ze.unwrap_or(out.len()));
        if s > e {
            return Err("E867: \\zs after \\ze".into());
        }
        out = format!("{}(?P<m>{}){}", &out[..s], &out[s..e], &out[e..]);
    }
    let insensitive = case.unwrap_or(ignorecase && !(smartcase && upper));
    let re = RegexBuilder::new(&out)
        .multi_line(true)
        .case_insensitive(insensitive)
        .build()
        .map_err(|e| format!("E383: Invalid search string: {pat} ({e})"))?;
    Ok(Pattern { re, group })
}

/// The text as a string, with a char → byte index.
pub struct Haystack {
    pub text: String,
    /// `bytes[c]`: where char `c` starts (and the text's length at the end).
    bytes: Vec<usize>,
}

impl Haystack {
    pub fn new(t: &dyn TextModel) -> Self {
        let text = t.slice(0..t.len_chars());
        let mut bytes: Vec<usize> = text.char_indices().map(|(b, _)| b).collect();
        bytes.push(text.len());
        Self { text, bytes }
    }

    /// The text of chars `from..to`.
    pub fn text_between(&self, from: Pos, to: Pos) -> String {
        self.text[self.byte(from)..self.byte(to.max(from))].to_string()
    }

    /// As `new`, with a line break after the last line, as Vim's buffers
    /// have (so `:s/\n//` sees one there).
    pub fn with_final_newline(t: &dyn TextModel) -> Self {
        let mut h = Self::new(t);
        h.text.push('\n');
        h.bytes.push(h.text.len());
        h
    }

    fn byte(&self, c: Pos) -> usize {
        self.bytes[c.min(self.bytes.len() - 1)]
    }

    fn char(&self, b: usize) -> Pos {
        self.bytes.partition_point(|&x| x < b)
    }
}

impl Pattern {
    /// The first match starting at or after char `from`, with its groups:
    /// `[0]` the match, `[1..=9]` what `\(...\)` caught (for `\1`).
    pub fn captures_at(&self, h: &Haystack, from: Pos) -> Option<Vec<Option<(Pos, Pos)>>> {
        let at = h.byte(from);
        if at > h.text.len() {
            return None;
        }
        let mut b = at;
        let caps = loop {
            let caps = self.re.captures_at(&h.text, b)?;
            if !self.group || caps.name("m").is_some() {
                break caps;
            }
            b = caps.get(0)?.start() + 1;
            while !h.text.is_char_boundary(b) {
                b += 1;
            }
        };
        let span = |m: Option<regex::Match>| m.map(|m| (h.char(m.start()), h.char(m.end())));
        let whole = if self.group {
            caps.name("m")
        } else {
            caps.get(0)
        };
        let mut out = vec![span(whole)];
        for i in 1..=9 {
            out.push(span(caps.name(&format!("g{i}"))));
        }
        Some(out)
    }

    /// The first match found starting at or after char `from`: its chars.
    pub fn find_at(&self, h: &Haystack, from: Pos) -> Option<(Pos, Pos)> {
        let at = h.byte(from);
        if at > h.text.len() {
            return None;
        }
        let (s, e) = if self.group {
            // The whole pattern may match further on than its `m` part.
            let mut b = at;
            loop {
                let caps = self.re.captures_at(&h.text, b)?;
                if let Some(m) = caps.name("m") {
                    break (m.start(), m.end());
                }
                b = caps.get(0)?.start() + 1;
                while !h.text.is_char_boundary(b) {
                    b += 1;
                }
            }
        } else {
            let m = self.re.find_at(&h.text, at)?;
            (m.start(), m.end())
        };
        Some((h.char(s), h.char(e)))
    }

    /// Non-overlapping matches in chars `from..to`, for highlighting.
    pub fn all(&self, h: &Haystack, from: Pos, to: Pos) -> Vec<(Pos, Pos)> {
        let mut out = Vec::new();
        let mut at = from;
        while at <= to {
            let Some((s, e)) = self.find_at(h, at) else {
                break;
            };
            if s >= to {
                break;
            }
            if e > s {
                out.push((s, e));
            }
            at = if e > s { e } else { s + 1 };
        }
        out
    }
}

/// Where a search found its match, and whether it wrapped around the end.
pub struct Found {
    pub start: Pos,
    pub end: Pos,
    pub wrapped: bool,
}

impl Pattern {
    /// The matches starting on `line`, as Vim finds them (with 'cpo' flag
    /// `c`): from the line's start, each one after the end of the last. A
    /// match running on into the next line is the line's last.
    fn line_matches(&self, t: &dyn TextModel, h: &Haystack, line: usize) -> Vec<(Pos, Pos)> {
        let start = t.line_to_char(line);
        let end = start + line_len(t, line);
        let mut out = Vec::new();
        let mut at = start;
        while at <= end {
            let Some((s, e)) = self.find_at(h, at) else {
                break;
            };
            if s > end {
                break;
            }
            out.push((s, e));
            if e > end {
                break;
            }
            at = if e > s { e } else { s + 1 };
            if at > end || (e == end && e > s) {
                break;
            }
        }
        out
    }
}

/// The `count`th match after (`forward`) or before `from`, as Vim's
/// searchit(). `at_end`: compare matches by their last char (the `e`
/// offset). `wrap`: go round the end of the text ('wrapscan').
#[allow(clippy::too_many_arguments)]
pub fn find(
    t: &dyn TextModel,
    h: &Haystack,
    pat: &Pattern,
    from: Pos,
    forward: bool,
    count: usize,
    at_end: bool,
    wrap: bool,
) -> Option<Found> {
    let lines = t.len_lines();
    let mut pos = from;
    let mut wrapped = false;
    let mut found = None;
    let key = |(s, e): (Pos, Pos)| -> Pos { if at_end && e > s { e - 1 } else { s } };
    for _ in 0..count {
        let line = t.char_to_line(pos.min(t.len_chars()));
        let col = (pos - t.line_to_char(line)) as isize;
        let len = line_len(t, line);
        let mut hit = None;
        if forward {
            // On the first line, only a match past the cursor; one on the
            // line's end counts as on its last char, so `n` after `/$`
            // moves on.
            for (i, l) in (line..lines).chain(0..=line).enumerate() {
                let first = i == 0;
                let ms = pat.line_matches(t, h, l);
                let m = ms.into_iter().find(|&m| {
                    if !first {
                        return true;
                    }
                    let k = key(m);
                    let kl = t.char_to_line(k.min(t.len_chars()));
                    if kl != line {
                        return kl > line;
                    }
                    let kc = (k - t.line_to_char(kl)) as isize;
                    if at_end {
                        kc > col
                    } else {
                        kc - isize::from(kc as usize == len && len > 0) > col
                    }
                });
                if let Some(m) = m {
                    if i > 0 && l <= line {
                        if !wrap {
                            return None;
                        }
                        wrapped = true;
                    }
                    hit = Some(m);
                    break;
                }
            }
        } else {
            // Backwards: the last match before the cursor on its line (from
            // the line before if it's in column 0), else the last on an
            // earlier line.
            let start_line = if col == 0 {
                line.checked_sub(1)
            } else {
                Some(line)
            };
            let order: Vec<usize> = match start_line {
                Some(sl) => (0..=sl).rev().chain((sl..lines).rev()).collect(),
                None => (0..lines).rev().collect(),
            };
            for (i, &l) in order.iter().enumerate() {
                let first = i == 0 && start_line == Some(line);
                let ms = pat.line_matches(t, h, l);
                let m = ms.into_iter().filter(|&m| {
                    if !first {
                        return true;
                    }
                    let k = key(m);
                    let kl = t.char_to_line(k.min(t.len_chars()));
                    kl < line || (kl == line && ((k - t.line_to_char(kl)) as isize) < col)
                });
                if let Some(m) = m.last() {
                    if start_line.is_none_or(|sl| l > sl) {
                        if !wrap {
                            return None;
                        }
                        wrapped = true;
                    }
                    hit = Some(m);
                    break;
                }
            }
        }
        let m = hit?;
        pos = key(m);
        found = Some(m);
    }
    let (start, end) = found?;
    Some(Found {
        start,
        end,
        wrapped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ropey::Rope;

    fn matches(pat: &str, text: &str) -> Vec<String> {
        let p = compile(pat, false, false).unwrap();
        let t = Rope::from_str(text);
        let h = Haystack::new(&t);
        p.all(&h, 0, t.len_chars())
            .into_iter()
            .map(|(s, e)| t.slice(s..e).to_string())
            .collect()
    }

    #[test]
    fn translates_vims_magic() {
        assert_eq!(matches("o\\+", "foo bo"), ["oo", "o"]);
        assert_eq!(matches("\\<the\\>", "the other theme the"), ["the", "the"]);
        assert_eq!(matches("a\\{2}", "a aa aaa"), ["aa", "aa"]);
        assert_eq!(matches("a\\{-1,}", "aaa"), ["a", "a", "a"]);
        assert_eq!(matches("\\(ab\\|cd\\)x", "abx cdx"), ["abx", "cdx"]);
        assert_eq!(matches("^a", "a\na ba"), ["a", "a"]);
        assert_eq!(matches("a$", "ba\na b"), ["a"]);
        assert_eq!(matches("a.c", "abc a.c"), ["abc", "a.c"]);
        assert_eq!(matches("a\\.c", "abc a.c"), ["a.c"]);
        assert_eq!(matches("1+2", "1+2 12"), ["1+2"]);
        assert_eq!(matches("[^a ]", "ab a\nc"), ["b", "c"]);
        assert_eq!(matches("\\s\\+", "a  b\n c"), ["  ", " "]);
        assert_eq!(matches("\\cTHE", "the The"), ["the", "The"]);
        assert_eq!(matches("foo\\zsbar", "foobar bar"), ["bar"]);
        assert_eq!(matches("\\v(a|b)+", "abba c"), ["abba"]);
        assert_eq!(matches("\\Va.c", "abc a.c"), ["a.c"]);
        assert_eq!(matches("*a", "*a a"), ["*a"]);
    }

    #[test]
    fn smartcase_looks_for_capitals() {
        let t = Rope::from_str("the The");
        let h = Haystack::new(&t);
        assert_eq!(compile("the", true, true).unwrap().all(&h, 0, 7).len(), 2);
        assert_eq!(compile("The", true, true).unwrap().all(&h, 0, 7).len(), 1);
    }
}
