//! Text objects and sentences, following Vim's textobject.c: `iw aw iW aW`,
//! `is as`, `ip ap`, the bracket blocks, the quotes and `it at`, and the
//! sentence search behind `(`/`)`. Positions are (line, col) as in motion.rs.

use crate::motion::{Cur, bck_word, bckend_word, dec, end_word, fwd_word, inc};
use crate::text::{self, char_at, class, is_blank, last_line, line_len, line_text};
use crate::{Pos, SyntaxObject, TextModel};
use std::ops::Range;

/// A visual selection a text object starts from, or extends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vis {
    /// The selection's other end (the cursor is the one end).
    pub anchor: Cur,
    /// Visual line mode.
    pub linewise: bool,
}

/// A text object, or where the cursor was left when there's none (Vim
/// moves it while looking and doesn't put it back).
pub type Found = Result<Object, Stop>;

/// Where a failed text object left the cursor, and the visual anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stop {
    pub cursor: Cur,
    pub anchor: Option<Cur>,
}

impl From<Cur> for Stop {
    fn from(cursor: Cur) -> Self {
        Stop {
            cursor,
            anchor: None,
        }
    }
}

/// What a text object selects: from `start` to `end`, and how. In visual
/// mode `start` is the selection's new anchor and `end` its new cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Object {
    pub start: Cur,
    pub end: Cur,
    /// Include the char at `end` (else stop before it).
    pub inclusive: bool,
    pub linewise: bool,
}

fn ch(t: &dyn TextModel, c: Cur) -> Option<char> {
    char_at(t, c.line, c.col)
}

fn cls(t: &dyn TextModel, c: Cur, big: bool) -> u32 {
    class(ch(t, c), big)
}

/// `inc`, but skipping the end-of-line position of a non-empty line.
pub fn incl(t: &dyn TextModel, c: &mut Cur) -> i32 {
    let r = inc(t, c);
    if r >= 1 && c.col > 0 { inc(t, c) } else { r }
}

/// `dec`, but skipping to the last char when crossing to a non-empty line.
pub fn decl(t: &dyn TextModel, c: &mut Cur) -> i32 {
    let r = dec(t, c);
    if r == 1 && c.col > 0 { dec(t, c) } else { r }
}

fn oneleft(c: &mut Cur) -> bool {
    if c.col == 0 {
        return false;
    }
    c.col -= 1;
    true
}

fn line_white(t: &dyn TextModel, line: usize) -> bool {
    line_text(t, line).chars().all(is_blank)
}

fn line_empty(t: &dyn TextModel, line: usize) -> bool {
    line_len(t, line) == 0
}

// ── Words ────────────────────────────────────────────────────────────────

/// Back to the start of the word or blanks under the cursor, on its line.
fn back_in_line(t: &dyn TextModel, c: &mut Cur, big: bool) {
    let sclass = cls(t, *c, big);
    while c.col > 0 {
        let mut p = *c;
        dec(t, &mut p);
        if cls(t, p, big) != sclass {
            break;
        }
        *c = p;
    }
}

/// `iw`/`aw` (`big`: `iW`/`aW`), `count` words (Vim's current_word).
pub fn word(
    t: &dyn TextModel,
    cursor: Cur,
    vis: Option<Vis>,
    count: usize,
    include: bool,
    big: bool,
) -> Found {
    let visual = vis.is_some();
    let mut anchor = vis.map_or(cursor, |v| v.anchor);
    let mut c = cursor;
    let mut start = cursor;
    let mut count = count;
    let mut inclusive = true;
    let mut include_white = false;
    if !visual || anchor == cursor {
        back_in_line(t, &mut c, big);
        start = c;
        if (cls(t, c, big) == 0) == include {
            let (to, ok) = end_word(t, c, 1, big, true, true);
            if !ok {
                return Err(c.into());
            }
            c = to;
        } else {
            let (to, _) = fwd_word(t, c, 1, big, true);
            c = to;
            if c.col == 0 {
                decl(t, &mut c);
            } else {
                oneleft(&mut c);
            }
            include_white = include;
        }
        anchor = start;
        count -= 1;
    }
    let fail = |c: Cur| {
        Err(Stop {
            cursor: c,
            anchor: visual.then_some(anchor),
        })
    };
    while count > 0 {
        inclusive = true;
        if visual && c < anchor {
            // Selecting backwards: extend backwards.
            if decl(t, &mut c) == -1 {
                return fail(c);
            }
            if include != (cls(t, c, big) != 0) {
                let (to, ok) = bck_word(t, c, 1, big, true);
                c = to;
                if !ok {
                    return fail(c);
                }
            } else {
                let (to, ok) = bckend_word(t, c, 1, big, true);
                c = to;
                if !ok {
                    return fail(c);
                }
                incl(t, &mut c);
            }
        } else {
            if incl(t, &mut c) == -1 {
                return fail(c);
            }
            if include != (cls(t, c, big) == 0) {
                let (to, ok) = fwd_word(t, c, 1, big, true);
                c = to;
                if !ok && count > 1 {
                    return fail(c);
                }
                if !oneleft(&mut c) {
                    inclusive = false;
                }
            } else {
                let (to, ok) = end_word(t, c, 1, big, true, true);
                c = to;
                if !ok {
                    return fail(c);
                }
            }
        }
        count -= 1;
    }
    if include_white && (cls(t, c, big) != 0 || (c.col == 0 && !inclusive)) {
        // No blanks after the word(s): take the blanks before, but not indent.
        let mut p = start;
        if oneleft(&mut p) {
            back_in_line(t, &mut p, big);
            if cls(t, p, big) == 0 && p.col > 0 {
                start = p;
                anchor = p;
            }
        }
    }
    let start = if visual { anchor } else { start };
    Ok(Object {
        start,
        end: c,
        inclusive,
        linewise: false,
    })
}

// ── Sentences ────────────────────────────────────────────────────────────

fn start_ps(t: &dyn TextModel, line: usize) -> bool {
    text::start_ps(t, line)
}

/// The start of the next (`forward`) or current/previous sentence (Vim's
/// findsent): what `)` and `(` move to.
pub fn find_sentence(t: &dyn TextModel, from: Cur, count: usize, forward: bool) -> Option<Cur> {
    let found = find_sentence_inner(t, from, count, forward);
    if found.is_some() {
        // Vim's findsent() sets the jump mark where it starts from.
        SENTENCE_JUMPS.with(|j| j.borrow_mut().push(from));
    }
    found
}

thread_local! {
    /// Where the sentence searches since the last take started from: each
    /// is a jump in Vim (for `''`), even inside `is`/`as`.
    static SENTENCE_JUMPS: std::cell::RefCell<Vec<Cur>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The sentence searches' starting points since the last call.
pub fn take_sentence_jumps() -> Vec<Cur> {
    SENTENCE_JUMPS.with(|j| std::mem::take(&mut *j.borrow_mut()))
}

fn find_sentence_inner(t: &dyn TextModel, from: Cur, count: usize, forward: bool) -> Option<Cur> {
    let mut pos = from;
    let step = |t: &dyn TextModel, p: &mut Cur| if forward { incl(t, p) } else { decl(t, p) };
    for n in (0..count).rev() {
        let mut noskip = false;
        'found: {
            if ch(t, pos).is_none() {
                // On an empty line (or a line's end): skip to a non-empty line.
                loop {
                    if step(t, &mut pos) == -1 {
                        break;
                    }
                    if ch(t, pos).is_some() {
                        break;
                    }
                }
                if forward {
                    break 'found;
                }
            } else if forward && pos.col == 0 && start_ps(t, pos.line) {
                if pos.line == last_line(t) {
                    return None;
                }
                pos.line += 1;
                break 'found;
            } else if !forward {
                decl(t, &mut pos);
            }
            // Back over white space and sentence-ending punctuation.
            let mut found_dot = false;
            while let Some(c) = ch(t, pos).filter(|&c| is_blank(c) || ".!?)]\"'".contains(c)) {
                let mut tpos = pos;
                if decl(t, &mut tpos) == -1 || (line_empty(t, tpos.line) && forward) {
                    break;
                }
                if found_dot {
                    break;
                }
                if ".!?".contains(c) {
                    found_dot = true;
                }
                if ")]\"'".contains(c) && !ch(t, tpos).is_some_and(|p| ".!?)]\"'".contains(p)) {
                    break;
                }
                decl(t, &mut pos);
            }
            let start_line = pos.line;
            loop {
                let c = ch(t, pos);
                if c.is_none() || (pos.col == 0 && start_ps(t, pos.line)) {
                    if !forward && pos.line != start_line {
                        pos.line += 1;
                        pos.col = 0;
                    }
                    break;
                }
                if matches!(c, Some('.' | '!' | '?')) {
                    let mut tpos = pos;
                    let mut end_of_text = false;
                    loop {
                        if inc(t, &mut tpos) == -1 {
                            end_of_text = true;
                            break;
                        }
                        if !ch(t, tpos).is_some_and(|c| ")]\"'".contains(c)) {
                            break;
                        }
                    }
                    let after = ch(t, tpos);
                    if end_of_text || after.is_none() || after.is_some_and(is_blank) {
                        pos = tpos;
                        if ch(t, pos).is_none() {
                            inc(t, &mut pos);
                        }
                        break;
                    }
                }
                if step(t, &mut pos) == -1 {
                    if n > 0 {
                        return None;
                    }
                    noskip = true;
                    break;
                }
            }
        }
        // Skip white space.
        while !noskip && ch(t, pos).is_some_and(is_blank) {
            if incl(t, &mut pos) == -1 {
                break;
            }
        }
    }
    Some(pos)
}

/// Back to the first of the blanks before `p` (Vim's find_first_blank).
fn first_blank(t: &dyn TextModel, p: &mut Cur) {
    while decl(t, p) != -1 {
        if !ch(t, *p).is_some_and(is_blank) {
            incl(t, p);
            break;
        }
    }
}

fn sentence_forward(t: &dyn TextModel, c: &mut Cur, count: usize, mut at_start: bool) {
    for n in (0..count).rev() {
        if let Some(p) = find_sentence(t, *c, 1, true) {
            *c = p;
        }
        if at_start {
            first_blank(t, c);
        }
        if n == 0 || at_start {
            decl(t, c);
        }
        at_start = !at_start;
    }
}

/// Extend a visual selection by sentences (Vim's current_sent, `extend:`).
/// `orig`: the cursor; `c`: where the next sentence starts; `pos`: `orig`
/// moved past any blanks.
fn extend_sentence(
    t: &dyn TextModel,
    orig: Cur,
    anchor: Cur,
    mut c: Cur,
    mut pos: Cur,
    count: usize,
    include: bool,
) -> Found {
    let count = if include { count * 2 } else { count };
    if orig < anchor {
        // The cursor is at the selection's start: extend backwards.
        let mut at_start = true;
        while pos < c {
            if !ch(t, pos).is_some_and(is_blank) {
                at_start = false;
                break;
            }
            incl(t, &mut pos);
        }
        if !at_start {
            c = find_sentence(t, c, 1, false).ok_or(c)?;
            if c == orig {
                at_start = true;
            } else {
                c = find_sentence(t, c, 1, true).ok_or(c)?;
            }
        }
        for _ in 0..count {
            if at_start {
                first_blank(t, &mut c);
            }
            if !at_start || (!include && !ch(t, c).is_some_and(is_blank)) {
                c = find_sentence(t, c, 1, false).ok_or(c)?;
            }
            at_start = !at_start;
        }
    } else {
        incl(t, &mut pos);
        let mut at_start = true;
        if pos != c {
            at_start = false;
            while pos < c {
                if !ch(t, pos).is_some_and(is_blank) {
                    at_start = true;
                    break;
                }
                incl(t, &mut pos);
            }
            if at_start {
                c = find_sentence(t, c, 1, false).ok_or(c)?;
            } else {
                c = orig;
            }
        }
        sentence_forward(t, &mut c, count, at_start);
    }
    Ok(Object {
        start: anchor,
        end: c,
        inclusive: true,
        linewise: false,
    })
}

/// `is`/`as` (Vim's current_sent).
pub fn sentence(
    t: &dyn TextModel,
    cursor: Cur,
    vis: Option<Vis>,
    count: usize,
    include: bool,
) -> Found {
    let mut start = cursor;
    let mut pos = start;
    let mut c = find_sentence(t, cursor, 1, true).ok_or(cursor)?;
    if let Some(v) = vis
        && v.anchor != cursor
    {
        return extend_sentence(t, cursor, v.anchor, c, pos, count, include);
    }
    while ch(t, pos).is_some_and(is_blank) {
        incl(t, &mut pos);
    }
    let start_blank;
    if pos == c {
        start_blank = true;
        first_blank(t, &mut start);
    } else {
        start_blank = false;
        start = find_sentence(t, c, 1, false).ok_or(c)?;
        c = start;
    }
    let ncount = if include {
        count * 2
    } else {
        count - usize::from(start_blank)
    };
    if ncount > 0 {
        sentence_forward(t, &mut c, ncount, true);
    } else {
        decl(t, &mut c);
    }
    if include {
        if start_blank {
            first_blank(t, &mut c);
            if ch(t, c).is_some_and(is_blank) {
                decl(t, &mut c);
            }
        } else if !ch(t, c).is_some_and(is_blank) {
            first_blank(t, &mut start);
        }
    }
    if vis.is_some() {
        if start == c {
            // Don't get stuck on a single blank before a sentence.
            let anchor = vis.map_or(start, |v| v.anchor);
            return extend_sentence(t, start, anchor, c, pos, count, include);
        }
        return Ok(Object {
            start,
            end: c,
            inclusive: true,
            linewise: false,
        });
    }
    // Take a line break after the sentence, if there is one.
    let mut end = c;
    let inclusive = incl(t, &mut end) == -1;
    if inclusive {
        end = c;
    }
    Ok(Object {
        start,
        end,
        inclusive,
        linewise: false,
    })
}

// ── Paragraphs ───────────────────────────────────────────────────────────

/// Extend a visual selection by paragraphs (Vim's current_par, `extend:`).
fn extend_paragraph(
    t: &dyn TextModel,
    mut line: usize,
    anchor: Cur,
    count: usize,
    include: bool,
) -> Found {
    let last = last_line(t);
    let back = line < anchor.line;
    let edge = if back { 0 } else { last };
    let step = |l: usize| if back { l - 1 } else { l + 1 };
    let mut ok = true;
    for _ in 0..count {
        if line == edge {
            ok = false;
            break;
        }
        let mut prev_white = None;
        for _ in 0..2 {
            line = step(line);
            let white = line_white(t, line);
            if prev_white == Some(white) {
                line = if back { line + 1 } else { line - 1 };
                break;
            }
            while line != edge {
                let next = step(line);
                if white != line_white(t, next)
                    || (!white && start_ps(t, if back { line } else { line + 1 }))
                {
                    break;
                }
                line = next;
            }
            if !include || line == edge {
                break;
            }
            prev_white = Some(white);
        }
    }
    let end = Cur::new(line, 0);
    if ok {
        Ok(Object {
            start: anchor,
            end,
            inclusive: false,
            linewise: true,
        })
    } else {
        Err(end.into())
    }
}

/// `ip`/`ap` (Vim's current_par): whole lines.
pub fn paragraph(
    t: &dyn TextModel,
    cursor: Cur,
    vis: Option<Vis>,
    count: usize,
    include: bool,
) -> Found {
    let last = last_line(t);
    if let Some(v) = vis
        && v.anchor.line != cursor.line
    {
        return extend_paragraph(t, cursor.line, v.anchor, count, include);
    }
    let mut start = cursor.line;
    let white_in_front = line_white(t, start);
    while start > 0 {
        if white_in_front {
            if !line_white(t, start - 1) {
                break;
            }
        } else if line_white(t, start - 1) || start_ps(t, start) {
            break;
        }
        start -= 1;
    }
    let mut end = start;
    while end <= last && line_white(t, end) {
        end += 1;
    }
    let mut end = end as i64 - 1;
    let mut i = count as i64;
    if !include && white_in_front {
        i -= 1;
    }
    while i > 0 {
        i -= 1;
        if end == last as i64 {
            return Err(cursor.into());
        }
        let do_white = !include && line_white(t, (end + 1) as usize);
        if include || !do_white {
            end += 1;
            while end < last as i64
                && !line_white(t, (end + 1) as usize)
                && !start_ps(t, (end + 1) as usize)
            {
                end += 1;
            }
        }
        if i == 0 && white_in_front && include {
            break;
        }
        if include || do_white {
            while end < last as i64 && line_white(t, (end + 1) as usize) {
                end += 1;
            }
        }
    }
    let end = end.max(0) as usize;
    if !white_in_front && !line_white(t, end) && include {
        while start > 0 && line_white(t, start - 1) {
            start -= 1;
        }
    }
    if let Some(v) = vis {
        if v.linewise && start == end && start == cursor.line {
            // `Vipip` on one white line would select it again: extend.
            return extend_paragraph(t, cursor.line, v.anchor, count, include);
        }
        let anchor = if v.anchor.line != start {
            Cur::new(start, 0)
        } else {
            v.anchor
        };
        return Ok(Object {
            start: anchor,
            end: Cur::new(end, 0),
            inclusive: false,
            linewise: true,
        });
    }
    Ok(Object {
        start: Cur::new(start, 0),
        end: Cur::new(end, 0),
        inclusive: false,
        linewise: true,
    })
}

// ── Brackets ─────────────────────────────────────────────────────────────

/// The unmatched `open` before `from` (backward) or `close` after it.
fn find_unmatched(
    t: &dyn TextModel,
    from: Cur,
    open: char,
    close: char,
    backward: bool,
) -> Option<Cur> {
    let (want, other) = if backward {
        (open, close)
    } else {
        (close, open)
    };
    let mut depth = 0usize;
    let mut p = from;
    loop {
        let r = if backward {
            dec(t, &mut p)
        } else {
            inc(t, &mut p)
        };
        if r == -1 {
            return None;
        }
        match ch(t, p) {
            Some(c) if c == other => depth += 1,
            Some(c) if c == want => {
                if depth == 0 {
                    return Some(p);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
}

/// The next `open` after `from`, unless an unmatched `close` comes first.
fn seek(t: &dyn TextModel, from: Cur, open: char, close: char) -> Option<Cur> {
    let mut p = from;
    while inc(t, &mut p) != -1 {
        match ch(t, p) {
            Some(c) if c == open => return Some(p),
            Some(c) if c == close => return None,
            _ => {}
        }
    }
    None
}

/// Leading blanks cover the cursor and `extra` more columns (Vim's inindent).
fn in_indent(t: &dyn TextModel, c: Cur, extra: usize) -> bool {
    let lead = line_text(t, c.line)
        .chars()
        .take_while(|&ch| is_blank(ch))
        .count();
    lead >= c.col + extra
}

/// `i(`/`a(` and the other brackets (Vim's current_block).
pub fn block(
    t: &dyn TextModel,
    cursor: Cur,
    vis: Option<Vis>,
    count: usize,
    include: bool,
    open: char,
    close: char,
) -> Found {
    let old_pos = cursor;
    let mut c = cursor;
    let (mut old_start, mut old_end) = (cursor, cursor);
    match vis {
        Some(v) if v.anchor != cursor => {
            if v.anchor < cursor {
                old_start = v.anchor;
                c = v.anchor;
            } else {
                old_end = v.anchor;
            }
        }
        _ => {
            if open == '{' {
                // Ignore indent.
                while in_indent(t, c, 1) {
                    if inc(t, &mut c) != 0 {
                        break;
                    }
                }
            }
            if ch(t, c) == Some(open) {
                c.col += 1;
            }
        }
    }
    let mut start = match find_unmatched(t, c, open, close, true) {
        Some(mut p) => {
            for _ in 1..count {
                p = find_unmatched(t, p, open, close, true).ok_or(old_pos)?;
            }
            p
        }
        // Not inside one: Neovim takes the next one after the cursor (and
        // with a count, the one nested in that, and so on).
        None => {
            let mut p = c;
            for _ in 0..count {
                p = seek(t, p, open, close).ok_or(old_pos)?;
            }
            p
        }
    };
    let mut end = find_unmatched(t, start, open, close, false).ok_or(old_pos)?;
    let mut sol = false;
    if !include {
        loop {
            // Exclude the brackets (and a closing one's indent).
            incl(t, &mut start);
            sol = end.col == 0;
            decl(t, &mut end);
            while in_indent(t, end, 1) {
                sol = true;
                if decl(t, &mut end) != 0 {
                    break;
                }
            }
            // A selection no bigger than before grows to the enclosing block.
            if vis.is_some() && start >= old_start && old_end >= end && start != end {
                let mut p = old_start;
                decl(t, &mut p);
                start = find_unmatched(t, p, open, close, true).ok_or(old_pos)?;
                end = find_unmatched(t, start, open, close, false).ok_or(old_pos)?;
            } else {
                break;
            }
        }
    }
    if vis.is_some() {
        if end < start {
            return Err(old_pos.into());
        }
        if sol && ch(t, end).is_some() {
            // Take the line break.
            inc(t, &mut end);
        }
        return Ok(Object {
            start,
            end,
            inclusive: true,
            linewise: false,
        });
    }
    let mut inclusive = false;
    if sol {
        incl(t, &mut end);
    } else if start <= end {
        inclusive = true;
    } else {
        // Nothing between the brackets: nothing to operate on.
        end = start;
    }
    Ok(Object {
        start,
        end,
        inclusive,
        linewise: false,
    })
}

// ── Quotes ───────────────────────────────────────────────────────────────

fn next_quote(line: &[char], mut col: usize, q: char, escape: bool) -> Option<usize> {
    loop {
        let c = *line.get(col)?;
        if escape && c == '\\' {
            col += 1;
            line.get(col)?;
        } else if c == q {
            return Some(col);
        }
        col += 1;
    }
}

fn prev_quote(line: &[char], mut col: usize, q: char) -> usize {
    while col > 0 {
        col -= 1;
        let mut n = 0;
        while col > n && line[col - n - 1] == '\\' {
            n += 1;
        }
        if n % 2 == 1 {
            col -= n;
        } else if line[col] == q {
            break;
        }
    }
    col
}

/// `i"`/`a"` and the other quotes, on the cursor's line (Vim's current_quote).
pub fn quote(
    t: &dyn TextModel,
    cursor: Cur,
    count: usize,
    include: bool,
    q: char,
) -> Option<Object> {
    let line: Vec<char> = line_text(t, cursor.line).chars().collect();
    let first = cursor.col;
    let (mut start, mut end);
    if line.get(first) == Some(&q) {
        // On a quote: count from the line's start to tell opening from closing.
        start = 0;
        loop {
            start = next_quote(&line, start, q, false)?;
            if start > first {
                return None;
            }
            end = next_quote(&line, start + 1, q, true)?;
            if start <= first && first <= end {
                break;
            }
            start = end + 1;
        }
    } else {
        start = prev_quote(&line, first, q);
        if line.get(start) != Some(&q) {
            start = next_quote(&line, start, q, false)?;
        }
        end = next_quote(&line, start + 1, q, true)?;
    }
    if include {
        if line.get(end + 1).is_some_and(|&c| is_blank(c)) {
            while line.get(end + 1).is_some_and(|&c| is_blank(c)) {
                end += 1;
            }
        } else {
            while start > 0 && is_blank(line[start - 1]) {
                start -= 1;
            }
        }
    }
    if !include && count < 2 {
        start += 1;
    }
    let mut inclusive = false;
    let mut e = Cur::new(cursor.line, end);
    if (include || count > 1) && inc(t, &mut e) == 2 {
        inclusive = true;
    }
    Some(Object {
        start: Cur::new(cursor.line, start),
        end: e,
        inclusive,
        linewise: false,
    })
}

// ── Tags ─────────────────────────────────────────────────────────────────

/// `it`/`at`: the `count`th HTML-style element around the cursor, or
/// around the visual selection (growing it, as `vitit` does).
pub fn tag(t: &dyn TextModel, cursor: Cur, vis: Option<Vis>, count: usize, include: bool) -> Found {
    // Flatten the text to chars with their positions, to scan across lines.
    let mut chars = Vec::new();
    for line in 0..=last_line(t) {
        for (col, c) in line_text(t, line).chars().enumerate() {
            chars.push((c, Cur::new(line, col)));
        }
        chars.push(('\n', Cur::new(line, line_len(t, line))));
    }
    let index = |p: Cur| {
        chars
            .iter()
            .position(|(_, q)| *q >= p)
            .unwrap_or(chars.len() - 1)
    };
    let (lo, hi) = match vis {
        Some(v) => (index(v.anchor.min(cursor)), index(v.anchor.max(cursor))),
        None => (index(cursor), index(cursor)),
    };
    // Every tag: (start index, end index inclusive, name, closing?).
    let mut tags = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].0 == '<'
            && let Some(len) = chars[i + 1..].iter().position(|(c, _)| *c == '>')
        {
            let inner: String = chars[i + 1..i + 1 + len].iter().map(|(c, _)| c).collect();
            let closing = inner.starts_with('/');
            let name: String = inner
                .trim_start_matches('/')
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '-' || *c == ':')
                .collect();
            if !name.is_empty() && !inner.ends_with('/') {
                tags.push((i, i + 1 + len, name, closing));
            }
            i += len + 2;
            continue;
        }
        i += 1;
    }
    // Pair them up, and take those around the cursor, innermost first.
    let mut stack: Vec<usize> = Vec::new();
    let mut pairs = Vec::new();
    for (k, tg) in tags.iter().enumerate() {
        if tg.3 {
            if let Some(pos) = stack.iter().rposition(|&o| tags[o].2 == tg.2) {
                let o = stack[pos];
                stack.truncate(pos);
                pairs.push((o, k));
            }
        } else {
            stack.push(k);
        }
    }
    let mut around: Vec<(usize, usize)> = pairs
        .into_iter()
        .filter(|&(o, c)| tags[o].0 <= lo && hi <= tags[c].1)
        .collect();
    around.sort_by_key(|&(o, c)| tags[c].1 - tags[o].0);
    let &(o, c) = around.get(count - 1).ok_or(cursor)?;
    let outer = Object {
        start: chars[tags[o].0].1,
        end: chars[tags[c].1].1,
        inclusive: true,
        linewise: false,
    };
    if include {
        return Ok(outer);
    }
    // Inside: from after the opening tag to before the closing one, as
    // Vim's dec() does it (onto a line break, which is then taken).
    let start = chars[tags[o].1 + 1].1;
    let mut end = chars[tags[c].0].1;
    if start == end {
        return Ok(Object {
            start,
            end,
            inclusive: false,
            linewise: false,
        });
    }
    dec(t, &mut end);
    // The same selection again: take the tags too.
    if vis.is_some_and(|v| v.anchor.min(cursor) == start && v.anchor.max(cursor) == end) {
        return Ok(outer);
    }
    Ok(Object {
        start,
        end,
        inclusive: true,
        linewise: false,
    })
}

/// A syntax object (`if`, `ah`, ...): the smallest one around the cursor, or
/// the `count`th one out; with a selection, the smallest bigger than it; with
/// none around, the next one after. As nvim-treesitter-textobjects picks
/// them, with its look-ahead on.
pub fn syntax(
    t: &dyn TextModel,
    cursor: Cur,
    vis: Option<Vis>,
    count: usize,
    around: bool,
    kind: SyntaxObject,
) -> Found {
    let at = |c: Cur| text::pos(t, c.line, c.col.min(line_len(t, c.line)));
    let (lo, hi) = match vis {
        Some(v) => (at(v.anchor.min(cursor)), at(v.anchor.max(cursor))),
        None => (at(cursor), at(cursor)),
    };
    let selected = vis.is_some() && hi > lo;
    let mut objects: Vec<Range<Pos>> = t
        .syntax_objects(kind, !around, lo)
        .into_iter()
        .filter(|r| r.end > r.start)
        .collect();
    objects.sort_by_key(|r| (r.end - r.start, r.start));
    objects.dedup();
    let mut around_it = objects
        .iter()
        .filter(|r| r.start <= lo && hi < r.end && !(selected && r.start == lo && r.end == hi + 1));
    let found = match around_it.nth(count.max(1) - 1) {
        Some(r) => r.clone(),
        None => objects
            .iter()
            .filter(|r| r.start > lo)
            .min_by_key(|r| r.start)
            .cloned()
            .ok_or(cursor)?,
    };
    Ok(syntax_object(t, found))
}

/// A range as an object: whole lines as lines, else chars, without the line
/// break it ends in.
fn syntax_object(t: &dyn TextModel, r: Range<Pos>) -> Object {
    let (sl, sc) = text::line_col(t, r.start);
    let (el, ec) = text::line_col(t, r.end);
    if sc == 0 && (ec == 0 || r.end == t.len_chars()) && el > sl {
        let last = if ec == 0 { el - 1 } else { el };
        return Object {
            start: Cur::new(sl, 0),
            end: Cur::new(last, line_len(t, last).saturating_sub(1)),
            inclusive: true,
            linewise: true,
        };
    }
    let mut end = r.end;
    while end > r.start + 1 && t.char(end - 1) == '\n' {
        end -= 1;
    }
    let (el, ec) = text::line_col(t, end - 1);
    Object {
        start: Cur::new(sl, sc),
        end: Cur::new(el, ec),
        inclusive: true,
        linewise: false,
    }
}

/// Where the `count`th syntax object's start is after the cursor (or
/// before it), for `]f` `[f` and the like.
pub fn syntax_jump(
    t: &dyn TextModel,
    cursor: Cur,
    count: usize,
    forward: bool,
    kind: SyntaxObject,
) -> Option<Cur> {
    let here = text::pos(t, cursor.line, cursor.col.min(line_len(t, cursor.line)));
    let mut starts: Vec<Pos> = t
        .syntax_objects(kind, false, here)
        .into_iter()
        .map(|r| r.start)
        .collect();
    starts.sort();
    starts.dedup();
    let to = if forward {
        starts
            .into_iter()
            .filter(|&s| s > here)
            .nth(count.max(1) - 1)?
    } else {
        starts
            .into_iter()
            .rev()
            .filter(|&s| s < here)
            .nth(count.max(1) - 1)?
    };
    let (l, c) = text::line_col(t, to);
    Some(Cur::new(l, c))
}
