//! Marks and the jump list, as Neovim keeps them (its mark.c): named marks
//! `m{a-z}` `m{A-Z}`, the special ones (`'` `[` `]` `.` `^` `<` `>`), the
//! jumps CTRL-O and CTRL-I go through, and how edits move them all.
//!
//! Marks are kept as Vim keeps them, (line, column) with the column in
//! bytes: an edit that doesn't move a mark leaves it on the same byte, not
//! the same char (or part way into one). They're turned into chars where
//! they're set and read ([`Vim::mk`], [`Vim::unmk`]). A mark on a deleted
//! line is gone (as Vim's lnum 0).

use super::{Mode, Vim};
use crate::TextModel;
use crate::text::{self, last_line, line_len};

/// A mark's place: a line and a byte column (the column may be past the
/// line's end: `usize::MAX` for "the end", as Vim's MAXCOL).
pub(super) type Mark = (usize, usize);

/// Most jumps the list keeps (Vim's JUMPLISTSIZE).
const JUMPLIST_SIZE: usize = 100;

/// All the marks.
#[derive(Debug, Clone, Default)]
pub(super) struct Marks {
    /// `a`–`z` and `A`–`Z`.
    pub named: std::collections::HashMap<char, Mark>,
    /// `''`: where the last jump was from (Vim's pcmark), and the one
    /// before it, kept until the jump is known to have moved.
    pub pc: Option<Mark>,
    pub prev_pc: Option<Mark>,
    /// `'[` and `']`: the start and end of the last change or yank.
    pub op_start: Option<Mark>,
    pub op_end: Option<Mark>,
    /// `'.`: the last change; `'^`: where insert mode was left.
    pub last_change: Option<Mark>,
    pub last_insert: Option<Mark>,
    /// `'<` and `'>`: the last visual selection, and its mode.
    pub visual: Option<(Mark, Mark, Mode, usize)>,
    /// The jump list, oldest first, and where CTRL-O/CTRL-I are in it.
    pub jumps: Vec<Mark>,
    pub jump_idx: usize,
}

/// What kind of edit is coming, when the text alone can't tell (Vim's
/// operations work on lines where the engine works on chars).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EditHint {
    /// Whole lines deleted (`dd`, `dj`, `V..d`, `:d`).
    Lines,
    /// A linewise change (`cc`, `S`): the first line stays, the rest go.
    Change,
    /// A line opened above (`O`).
    Above,
    /// `:s`: lines it joins are deleted (their marks too), not moved.
    Substitute,
    /// `J` or a delete of chars: undo replaces the lines it spans.
    Spanned,
}

/// How an edit changed the lines: what Neovim's mark_adjust and
/// mark_col_adjust are told.
enum LineChange {
    /// Lines `first..=last` deleted (the ones after move up).
    Deleted { first: usize, last: usize },
    /// `count` lines inserted before line `at`.
    Inserted { at: usize, count: usize },
    /// Lines `first..=last` replaced by others, `delta` more (undo).
    Replaced {
        first: usize,
        last: usize,
        delta: isize,
    },
    /// Lines joined: line `to` (the ones between deleted) onto line `into`,
    /// its marks' columns moved by `shift` (Vim's mark_col_adjust, and its
    /// `spaces_removed` for leading blanks dropped by `J`).
    Joined {
        into: usize,
        from: usize,
        to: usize,
        shift: isize,
        spaces_removed: isize,
    },
}

/// A column in chars as bytes into its line (past the end, each char
/// missing counts one; `usize::MAX` stays).
fn col_to_byte(t: &dyn TextModel, l: usize, c: usize) -> usize {
    if c == usize::MAX || l > last_line(t) {
        return c;
    }
    let line = text::line_text(t, l);
    let len = line.chars().count();
    if c >= len {
        return line.len() + (c - len);
    }
    line.char_indices().nth(c).map_or(0, |(b, _)| b)
}

/// Back: the char a byte column is in (as Vim puts the cursor on a
/// mark part way into a char).
fn byte_to_col(t: &dyn TextModel, l: usize, b: usize) -> usize {
    if b == usize::MAX || l > last_line(t) {
        return b;
    }
    let line = text::line_text(t, l);
    if b >= line.len() {
        return line.chars().count() + (b - line.len());
    }
    line.char_indices().take_while(|&(i, _)| i <= b).count() - 1
}

impl Marks {
    /// Move every mark's line by `f` (for `:m`, which moves lines as a
    /// block and their marks with them).
    pub fn map_lines(&mut self, f: impl Fn(usize) -> usize) {
        let g = |m: &mut Option<Mark>| {
            if let Some((l, c)) = *m {
                *m = Some((f(l), c));
            }
        };
        for p in self.named.values_mut() {
            p.0 = f(p.0);
        }
        g(&mut self.pc);
        g(&mut self.prev_pc);
        g(&mut self.last_change);
        g(&mut self.last_insert);
        if let Some((a, b, m, w)) = self.visual {
            self.visual = Some(((f(a.0), a.1), (f(b.0), b.1), m, w));
        }
        for j in &mut self.jumps {
            j.0 = f(j.0);
        }
    }

    /// Every mark that moves with the text, for adjusting: named, the
    /// special ones, and the jump list (which keeps a mark on a deleted
    /// line, at the first line deleted).
    fn adjust(&mut self, change: &LineChange) {
        let one = |m: &mut Option<Mark>, keep: bool| {
            if let Some(p) = *m {
                *m = adjust_one(p, change, keep);
            }
        };
        let named: Vec<char> = self.named.keys().copied().collect();
        for c in named {
            let p = self.named[&c];
            match adjust_one(p, change, false) {
                Some(p) => {
                    self.named.insert(c, p);
                }
                None => {
                    self.named.remove(&c);
                }
            }
        }
        one(&mut self.pc, false);
        one(&mut self.prev_pc, false);
        one(&mut self.last_change, false);
        one(&mut self.last_insert, false);
        if let Some((a, b, m, w)) = self.visual {
            let a = adjust_one(a, change, true).unwrap_or(a);
            let b = adjust_one(b, change, true).unwrap_or(b);
            self.visual = Some((a, b, m, w));
        }
        for j in &mut self.jumps {
            *j = adjust_one(*j, change, true).unwrap_or(*j);
        }
    }
}

/// One mark after a change; None if its line went. `keep`: don't lose it,
/// put it on the first line deleted instead (Vim's ONE_ADJUST_NODEL).
fn adjust_one((l, c): Mark, change: &LineChange, keep: bool) -> Option<Mark> {
    match *change {
        LineChange::Deleted { first, last } => {
            let n = last - first + 1;
            if l >= first && l <= last {
                keep.then_some((first, c))
            } else if l > last {
                Some((l - n, c))
            } else {
                Some((l, c))
            }
        }
        LineChange::Inserted { at, count } => Some(if l >= at { (l + count, c) } else { (l, c) }),
        LineChange::Replaced { first, last, delta } => {
            if l >= first && l <= last {
                keep.then_some((first, c))
            } else if l > last {
                Some(((l as isize + delta) as usize, c))
            } else {
                Some((l, c))
            }
        }
        LineChange::Joined {
            into,
            from,
            to,
            shift,
            spaces_removed,
        } => {
            let n = to - from + 1;
            if l >= from && l <= to {
                // Only the last joined line's marks survive (the ones
                // between were deleted lines); they move onto `into`.
                // Marks kept on a deleted middle line (the jump list's) are
                // put on the next line first, so they move with it too.
                if l == to || keep {
                    let col = if shift < 0 && c as isize <= -shift {
                        0
                    } else if (c as isize) < spaces_removed {
                        (shift + spaces_removed) as usize
                    } else {
                        (c as isize + shift) as usize
                    };
                    Some((into, col))
                } else {
                    None
                }
            } else if l > to {
                Some((l - n, c))
            } else {
                Some((l, c))
            }
        }
    }
}

impl Vim {
    /// Marks after undoing or redoing an edit, as Vim's u_undoredo: when
    /// the number of lines changes, marks on the lines replaced go (the
    /// jump list keeps them, on the first) and the lines after move.
    /// `old` lines at `at` were replaced by `new`.
    pub(super) fn marks_after_undo(&mut self, at: usize, old: usize, new: usize) {
        if old != new {
            let change = if old == 0 {
                LineChange::Inserted { at, count: new }
            } else {
                LineChange::Replaced {
                    first: at,
                    last: at + old - 1,
                    delta: new as isize - old as isize,
                }
            };
            self.marks.adjust(&change);
        }
    }

    /// Keep the marks on their text after an edit of the chars from
    /// (`l1`,`c1`) to (`l2`,`c2`) (as they were) into `with`: columns and
    /// the lengths of lines `l1`, `l2` in bytes, as the marks are then.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn marks_after_edit(
        &mut self,
        l1: usize,
        c1: usize,
        l2: usize,
        c2: usize,
        old_len1: usize,
        old_len2: usize,
        removed: &str,
        with: &str,
        lines_after: usize,
    ) {
        let d = l2 - l1;
        let k = with.matches('\n').count();
        let hint = self.edit_hint.take();
        let change = match hint {
            Some(EditHint::Lines) if d > 0 => {
                Some(if c1 == 0 && lines_after == 1 && l1 == 0 && c2 > 0 {
                    // The whole text.
                    LineChange::Deleted { first: 0, last: l2 }
                } else if c1 == 0 && c2 == 0 {
                    LineChange::Deleted {
                        first: l1,
                        last: l2 - 1,
                    }
                } else {
                    // The last lines, with the line break before them.
                    LineChange::Deleted {
                        first: l1 + 1,
                        last: l2,
                    }
                })
            }
            Some(EditHint::Change) if d > 0 => Some(LineChange::Deleted {
                first: l1 + 1,
                last: l2,
            }),
            Some(EditHint::Above) => Some(LineChange::Inserted { at: l1, count: k }),
            Some(EditHint::Substitute) if d > 0 && k == 0 => Some(LineChange::Deleted {
                first: l1 + 1,
                last: l2,
            }),
            _ if d == 0 && k == 0 => None,
            _ if d == 0 => {
                // Lines added: before this one if put in at its start (a
                // linewise `P`), else after it (marks on a split line stay).
                if c1 == 0 && with.ends_with('\n') && !with.starts_with('\n') {
                    Some(LineChange::Inserted { at: l1, count: k })
                } else {
                    Some(LineChange::Inserted {
                        at: l1 + 1,
                        count: k,
                    })
                }
            }
            _ if k == 0 => {
                // Joined: line l2's rest comes after what's left of l1. A `J`
                // (a line break and indent, into a space or nothing) moves its
                // marks back by the blanks it dropped; a delete across lines
                // doesn't (Vim deletes the chars first, then joins).
                let first_part = c1 + with.len();
                let join_like = d == 1
                    && c1 == old_len1
                    && removed.starts_with('\n')
                    && removed[1..].chars().all(|c| c == ' ' || c == '\t')
                    && with.chars().all(|c| c == ' ');
                let (shift, spaces_removed) = if join_like {
                    // Blanks dropped less spaces put in (negative when `J`
                    // adds a space).
                    let removed = c2 as isize - with.len() as isize;
                    (c1 as isize - removed, removed)
                } else {
                    (first_part as isize, 0)
                };
                Some(LineChange::Joined {
                    into: l1,
                    from: l1 + 1,
                    to: l2,
                    shift,
                    spaces_removed,
                })
            }
            _ => None,
        };
        let _ = old_len2;
        // '.: Vim's changed_lines() for the lines deleted, or the line after a
        // join; else where the change starts.
        let last_change = match change {
            Some(LineChange::Deleted { first, .. }) if hint.is_some() => (first, 0),
            Some(LineChange::Joined { into, .. }) => (into + 1, 0),
            _ => (l1, c1),
        };
        if let Some(ch) = change {
            self.marks.adjust(&ch);
        }
        self.marks.last_change = Some(last_change);
    }

    // ── Setting and reading marks ────────────────────────────────────────

    /// A place in chars as a mark.
    pub(super) fn mk(&self, t: &dyn TextModel, (l, c): (usize, usize)) -> Mark {
        (l, col_to_byte(t, l, c))
    }

    /// A mark's place in chars.
    pub(super) fn unmk(&self, t: &dyn TextModel, (l, b): Mark) -> (usize, usize) {
        (l, byte_to_col(t, l, b))
    }

    fn here(&self, t: &dyn TextModel) -> Mark {
        self.mk(t, text::line_col(t, self.cursor.min(t.len_chars())))
    }

    /// Remember where a jump starts (setpcmark): for `''` and CTRL-O.
    pub(super) fn setpcmark(&mut self, t: &dyn TextModel) {
        let here = self.here(t);
        self.marks.prev_pc = self.marks.pc;
        self.marks.pc = Some(here);
        if self.marks.jumps.len() >= JUMPLIST_SIZE {
            self.marks.jumps.remove(0);
        }
        self.marks.jumps.push(here);
        self.marks.jump_idx = self.marks.jumps.len();
    }

    /// After a command: a "jump" that didn't move keeps the older `''`.
    pub(super) fn checkpcmark(&mut self, t: &dyn TextModel) {
        if let Some(prev) = self.marks.prev_pc.take()
            && (self.marks.pc == Some(self.here(t)) || self.marks.pc.is_none())
        {
            self.marks.pc = Some(prev);
        }
    }

    /// `m{char}`.
    pub(super) fn set_mark(&mut self, t: &dyn TextModel, c: char) -> bool {
        let here = self.here(t);
        match c {
            'a'..='z' | 'A'..='Z' => {
                self.marks.named.insert(c, here);
            }
            '\'' | '`' => {
                self.setpcmark(t);
                // Kept even if the cursor doesn't move.
                self.marks.prev_pc = self.marks.pc;
            }
            '[' => self.marks.op_start = Some(here),
            ']' => self.marks.op_end = Some(here),
            '<' | '>' => {
                let (a, b, m, w) = self.marks.visual.unwrap_or((here, here, Mode::Visual, 0));
                self.marks.visual = Some(if c == '<' {
                    (here, b, m, w)
                } else {
                    (a, here, m, w)
                });
            }
            _ => return false,
        }
        true
    }

    /// Where a mark is, in chars, if it's set (and its line still there).
    pub(super) fn mark(&self, t: &dyn TextModel, c: char) -> Option<(usize, usize)> {
        self.mark_any(c)
            .filter(|m| m.0 <= last_line(t))
            .map(|m| self.unmk(t, m))
    }

    fn mark_any(&self, c: char) -> Option<Mark> {
        let m = match c {
            'a'..='z' | 'A'..='Z' => self.marks.named.get(&c).copied(),
            '\'' | '`' => self.marks.pc,
            '[' => self.marks.op_start,
            ']' => self.marks.op_end,
            '.' => self.marks.last_change,
            '^' => self.marks.last_insert,
            '<' | '>' => self.marks.visual.map(|(a, b, mode, _)| {
                let (start, end) = if a <= b { (a, b) } else { (b, a) };
                match (c, mode) {
                    ('<', Mode::VisualLine) => (start.0, 0),
                    ('>', Mode::VisualLine) => (end.0, usize::MAX),
                    ('<', _) => start,
                    _ => end,
                }
            }),
            _ => None,
        }?;
        Some(m)
    }

    /// A mark for the app or tests: where it is, as (line, column) (a
    /// line past the end for `'.` after the last lines went).
    pub fn get_mark(&self, t: &dyn TextModel, c: char) -> Option<(usize, usize)> {
        self.mark_any(c).map(|m| self.unmk(t, m))
    }

    /// The jump list, oldest first, and where CTRL-O/CTRL-I are in it
    /// (tidied first, as Neovim's getjumplist() does).
    pub fn jumplist(&mut self, t: &dyn TextModel) -> (Vec<(usize, usize)>, usize) {
        self.cleanup_jumplist(t);
        let jumps = self.marks.jumps.iter().map(|&j| self.unmk(t, j)).collect();
        (jumps, self.marks.jump_idx)
    }

    /// Set a mark's place directly (for tests, and restoring a session).
    pub fn place_mark(&mut self, t: &dyn TextModel, c: char, at: (usize, usize)) {
        let at = self.mk(t, at);
        match c {
            '\'' | '`' => self.marks.pc = Some(at),
            c => {
                self.marks.named.insert(c, at);
            }
        }
    }

    /// The cursor to a mark's place (clamped onto its line).
    pub(super) fn to_mark(&self, t: &dyn TextModel, (l, c): (usize, usize)) -> usize {
        let visual = matches!(self.mode, Mode::Visual | Mode::VisualLine);
        let len = line_len(t, l);
        let max = if visual { len } else { len.saturating_sub(1) };
        text::pos(t, l, c.min(max))
    }

    // ── The jump list ────────────────────────────────────────────────────

    /// Drop duplicate lines from the jump list, keeping the newest, and a
    /// last entry on the cursor's line (cleanup_jumplist).
    fn cleanup_jumplist(&mut self, t: &dyn TextModel) {
        let jumps = std::mem::take(&mut self.marks.jumps);
        let mut kept = Vec::new();
        let mut idx = self.marks.jump_idx;
        let at_end = idx >= jumps.len();
        for (from, j) in jumps.iter().enumerate() {
            if idx == from {
                idx = kept.len();
            }
            let dup = jumps[from + 1..].iter().any(|k| k.0 == j.0);
            if !dup {
                kept.push(*j);
            }
        }
        if at_end {
            idx = kept.len();
        }
        let cl = self.here(t).0;
        if !kept.is_empty() && idx == kept.len() && kept.last().is_some_and(|j| j.0 == cl) {
            kept.pop();
            idx -= 1;
        }
        self.marks.jumps = kept;
        self.marks.jump_idx = idx;
    }

    /// CTRL-O (back, `count` negative) and CTRL-I: the jump `count` away
    /// (get_jumplist), or None.
    pub(super) fn jump(&mut self, t: &dyn TextModel, count: isize) -> Option<(usize, usize)> {
        self.cleanup_jumplist(t);
        let len = self.marks.jumps.len() as isize;
        if len == 0 {
            return None;
        }
        let idx = self.marks.jump_idx as isize;
        if idx + count < 0 || idx + count >= len {
            return None;
        }
        if self.marks.jump_idx == self.marks.jumps.len() {
            // The first CTRL-O after a jump: remember where it's from.
            self.setpcmark(t);
            self.marks.jump_idx -= 1;
            if self.marks.jump_idx as isize + count < 0 {
                return None;
            }
        }
        let i = (self.marks.jump_idx as isize + count) as usize;
        self.marks.jump_idx = i;
        let (l, c) = self.marks.jumps[i];
        let l = l.min(last_line(t));
        Some(self.unmk(t, (l, c)))
    }
}
