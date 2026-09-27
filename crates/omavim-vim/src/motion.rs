//! Vim's motion algorithms, following its search.c and textobject.c: the
//! word, word-end and paragraph motions, `%`, and `f`/`t`. Positions are
//! (line, col) where col may be the line's length: Vim's NUL at the end.

use crate::TextModel;
use crate::text::{char_at, class, last_line, line_len};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Cur {
    pub line: usize,
    pub col: usize,
}

impl Cur {
    pub fn new(line: usize, col: usize) -> Self {
        Self { line, col }
    }
}

/// One char forward. 0: moved; 2: moved onto the end of the line; 1: moved to
/// the next line; -1: at the end of the text, didn't move. (Vim's `inc()`.)
pub fn inc(t: &dyn TextModel, c: &mut Cur) -> i32 {
    let len = line_len(t, c.line);
    if c.col < len {
        c.col += 1;
        if c.col < len { 0 } else { 2 }
    } else if c.line < last_line(t) {
        c.line += 1;
        c.col = 0;
        1
    } else {
        -1
    }
}

/// One char back. 0: moved; 1: moved to the end of the previous line; -1: at
/// the start of the text. (Vim's `dec()`.)
pub fn dec(t: &dyn TextModel, c: &mut Cur) -> i32 {
    if c.col > 0 {
        c.col = c.col.min(line_len(t, c.line)) - 1;
        0
    } else if c.line > 0 {
        c.line -= 1;
        c.col = line_len(t, c.line);
        1
    } else {
        -1
    }
}

fn cls(t: &dyn TextModel, c: Cur, big: bool) -> u32 {
    class(char_at(t, c.line, c.col), big)
}

fn empty_line(t: &dyn TextModel, line: usize) -> bool {
    line_len(t, line) == 0
}

/// Skip chars of class `cclass`; true if the text ran out.
fn skip_chars(t: &dyn TextModel, c: &mut Cur, cclass: u32, forward: bool, big: bool) -> bool {
    while cls(t, *c, big) == cclass {
        let r = if forward { inc(t, c) } else { dec(t, c) };
        if r == -1 {
            return true;
        }
    }
    false
}

/// `w`/`W`. With `eol` (under an operator) the last word stops at the end of
/// its line. Returns the new position and whether it fully succeeded.
pub fn fwd_word(t: &dyn TextModel, mut c: Cur, count: usize, big: bool, eol: bool) -> (Cur, bool) {
    let mut count = count as i64;
    while {
        count -= 1;
        count >= 0
    } {
        let sclass = cls(t, c, big);
        let last = c.line == last_line(t);
        let i = inc(t, &mut c);
        if i == -1 || (i >= 1 && last) {
            return (c, false);
        }
        if i >= 1 && eol && count == 0 {
            return (c, true);
        }
        if sclass != 0 {
            while cls(t, c, big) == sclass {
                let i = inc(t, &mut c);
                if i == -1 || (i >= 1 && eol && count == 0) {
                    return (c, true);
                }
            }
        }
        while cls(t, c, big) == 0 {
            if c.col == 0 && empty_line(t, c.line) {
                break;
            }
            let i = inc(t, &mut c);
            if i == -1 || (i >= 1 && eol && count == 0) {
                return (c, true);
            }
        }
    }
    (c, true)
}

/// `e`/`E`. `stop`: stay at the end of the current word when already on its
/// last char (for `cw`). `empty`: stop on an empty line.
pub fn end_word(
    t: &dyn TextModel,
    mut c: Cur,
    count: usize,
    big: bool,
    mut stop: bool,
    empty: bool,
) -> (Cur, bool) {
    for _ in 0..count {
        let sclass = cls(t, c, big);
        if inc(t, &mut c) == -1 {
            return (c, false);
        }
        let mut finished = false;
        if cls(t, c, big) == sclass && sclass != 0 {
            if skip_chars(t, &mut c, sclass, true, big) {
                return (c, false);
            }
        } else if !stop || sclass == 0 {
            while cls(t, c, big) == 0 {
                if c.col == 0 && empty_line(t, c.line) && empty {
                    finished = true;
                    break;
                }
                if inc(t, &mut c) == -1 {
                    return (c, false);
                }
            }
            if !finished {
                let cl = cls(t, c, big);
                if skip_chars(t, &mut c, cl, true, big) {
                    return (c, false);
                }
            }
        }
        if !finished {
            dec(t, &mut c);
        }
        stop = false;
    }
    (c, true)
}

/// `b`/`B`.
pub fn bck_word(
    t: &dyn TextModel,
    mut c: Cur,
    count: usize,
    big: bool,
    mut stop: bool,
) -> (Cur, bool) {
    'count: for _ in 0..count {
        let sclass = cls(t, c, big);
        if dec(t, &mut c) == -1 {
            return (c, false);
        }
        if !stop || sclass == cls(t, c, big) || sclass == 0 {
            while cls(t, c, big) == 0 {
                if c.col == 0 && empty_line(t, c.line) {
                    stop = false;
                    continue 'count;
                }
                if dec(t, &mut c) == -1 {
                    return (c, true);
                }
            }
            let cl = cls(t, c, big);
            if skip_chars(t, &mut c, cl, false, big) {
                return (c, true);
            }
        }
        inc(t, &mut c);
        stop = false;
    }
    (c, true)
}

/// `ge`/`gE`.
pub fn bckend_word(
    t: &dyn TextModel,
    mut c: Cur,
    count: usize,
    big: bool,
    eol: bool,
) -> (Cur, bool) {
    for _ in 0..count {
        let sclass = cls(t, c, big);
        let i = dec(t, &mut c);
        if i == -1 {
            return (c, false);
        }
        if eol && i == 1 {
            return (c, true);
        }
        if sclass != 0 {
            while cls(t, c, big) == sclass {
                let i = dec(t, &mut c);
                if i == -1 || (eol && i == 1) {
                    return (c, true);
                }
            }
        }
        while cls(t, c, big) == 0 {
            if c.col == 0 && empty_line(t, c.line) {
                break;
            }
            let i = dec(t, &mut c);
            if i == -1 || (eol && i == 1) {
                return (c, true);
            }
        }
    }
    (c, true)
}

/// `}` (forward) and `{`: the next paragraph boundary (an empty line). Returns
/// the position and whether the motion becomes inclusive (Vim's `findpar`).
pub fn paragraph(
    t: &dyn TextModel,
    line: usize,
    count: usize,
    forward: bool,
) -> Option<(Cur, bool)> {
    let last = last_line(t) as i64;
    let dir: i64 = if forward { 1 } else { -1 };
    let mut curr = line as i64;
    let mut count = count;
    while count > 0 {
        count -= 1;
        let mut did_skip = false;
        let mut first = true;
        loop {
            if !empty_line(t, curr as usize) {
                did_skip = true;
            }
            if !first && did_skip && empty_line(t, curr as usize) {
                break;
            }
            curr += dir;
            if curr < 0 || curr > last {
                if count > 0 {
                    return None;
                }
                curr -= dir;
                break;
            }
            first = false;
        }
    }
    let curr = curr as usize;
    if curr as i64 == last && forward {
        let len = line_len(t, curr);
        if len != 0 {
            return Some((Cur::new(curr, len - 1), true));
        }
    }
    Some((Cur::new(curr, 0), false))
}

/// `%`: the first bracket at or after the cursor on its line, and its match.
pub fn match_pair(t: &dyn TextModel, c: Cur) -> Option<Cur> {
    let len = line_len(t, c.line);
    let (col, open) = (c.col..len).find_map(|col| {
        let ch = char_at(t, c.line, col)?;
        "(){}[]".contains(ch).then_some((col, ch))
    })?;
    let (want, forward) = match open {
        '(' => (')', true),
        '[' => (']', true),
        '{' => ('}', true),
        ')' => ('(', false),
        ']' => ('[', false),
        _ => ('{', false),
    };
    let mut depth = 0usize;
    let mut p = Cur::new(c.line, col);
    loop {
        let r = if forward {
            inc(t, &mut p)
        } else {
            dec(t, &mut p)
        };
        if r == -1 {
            return None;
        }
        match char_at(t, p.line, p.col) {
            Some(ch) if ch == open => depth += 1,
            Some(ch) if ch == want => {
                if depth == 0 {
                    return Some(p);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Find {
    /// f
    Forward,
    /// F
    Backward,
    /// t
    Till,
    /// T
    TillBack,
}

impl Find {
    pub fn forward(self) -> bool {
        matches!(self, Find::Forward | Find::Till)
    }
    pub fn till(self) -> bool {
        matches!(self, Find::Till | Find::TillBack)
    }
    pub fn reversed(self) -> Find {
        match self {
            Find::Forward => Find::Backward,
            Find::Backward => Find::Forward,
            Find::Till => Find::TillBack,
            Find::TillBack => Find::Till,
        }
    }
}

/// `f`/`F`/`t`/`T` on the cursor's line (Vim's `searchc`). `repeat` is `;`
/// or `,`: then a `t` right in front of its char skips it (Neovim's default
/// 'cpoptions' has no `;`).
pub fn find_char(
    t: &dyn TextModel,
    c: Cur,
    kind: Find,
    ch: char,
    count: usize,
    repeat: bool,
) -> Option<Cur> {
    let len = line_len(t, c.line);
    let mut col = c.col as i64;
    let dir: i64 = if kind.forward() { 1 } else { -1 };
    let mut stop = !(repeat && count == 1 && kind.till());
    for _ in 0..count {
        loop {
            col += dir;
            if col < 0 || col >= len as i64 {
                return None;
            }
            if char_at(t, c.line, col as usize) == Some(ch) && stop {
                break;
            }
            stop = true;
        }
    }
    if kind.till() {
        col -= dir;
    }
    Some(Cur::new(c.line, col as usize))
}
