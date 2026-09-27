//! The engine: keys in, edits and cursor moves out.
//!
//! Normal and visual mode commands are parsed from the keys typed so far
//! (`pending`): each key is added and the whole command parsed again, which
//! is either incomplete (wait), invalid (beep), or complete (run it). Every
//! edit goes through [`Vim::edit`], which records it for undo.

use crate::key::Key;
use crate::motion::{self, Cur, Find};
use crate::text::{
    self, char_at, first_non_blank, indent, is_blank, last_line, line_len, line_text,
};
use crate::{Mode, Pos, TextModel};

/// A command couldn't be done. Vim beeps and drops the keys typed after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Beep;

type R<T = ()> = Result<T, Beep>;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Register {
    pub text: String,
    pub linewise: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Motion {
    Left,
    Right,
    Up,
    Down,
    LineStart,
    FirstNonBlank,
    LineEnd,
    LastNonBlank,
    Column,
    GotoFirst,
    GotoLast,
    NextLine,
    PrevLine,
    CurrentLine,
    Word(bool),
    WordBack(bool),
    WordEnd(bool),
    WordEndBack(bool),
    Find(Find, char),
    RepeatFind(bool),
    ParagraphForward,
    ParagraphBack,
    Match,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Exclusive,
    Inclusive,
    Linewise,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Delete,
    Change,
    Yank,
    ShiftRight,
    ShiftLeft,
    Lower,
    Upper,
    Toggle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Insert {
    Before,
    After,
    LineStart,
    LineEnd,
    Below,
    Above,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Move(Motion),
    Operate(Op, Option<Motion>),
    Insert(Insert),
    Replace,
    ReplaceChar(char),
    Join(bool),
    ToggleCase,
    Put(bool),
    Visual(bool),
    Undo,
    Redo,
    Repeat,
    Cancel,
    // Visual mode only.
    SwapEnds,
}

#[derive(Debug, Clone, Copy)]
struct Command {
    /// Count before the operator and before the motion, multiplied.
    count: Option<usize>,
    action: Action,
}

enum Parse {
    Incomplete,
    Invalid,
    Done(Command),
}

#[derive(Debug, Clone)]
struct Edit {
    at: Pos,
    removed: String,
    inserted: String,
    /// The first line it changed, where undo puts the cursor (Vim's undo
    /// works in whole lines and knows this; it can't be recovered after).
    line: usize,
}

#[derive(Debug, Clone)]
struct Group {
    edits: Vec<Edit>,
    cursor_before: Pos,
}

/// The insert session under way.
#[derive(Debug, Clone)]
struct Session {
    kind: Option<Insert>,
    count: usize,
    /// Keys typed in it, for repeating it `count` times.
    typed: Vec<Key>,
    /// Where it started, for Ctrl-w and Ctrl-u stopping there once.
    start: Pos,
    /// The line has only its autoindent so far: Esc or Enter removes it.
    did_ai: bool,
    /// Replace mode: the chars overwritten, so Backspace can put them back.
    replaced: Vec<Option<char>>,
}

#[derive(Debug, Clone)]
enum LastChange {
    Keys {
        count: Option<usize>,
        keys: Vec<Key>,
    },
    /// A visual operator: the same extent from the cursor, then the operator.
    Visual {
        linewise: bool,
        lines: usize,
        last_col_or_chars: usize,
        keys: Vec<Key>,
    },
}

pub struct Vim {
    mode: Mode,
    cursor: Pos,
    /// The screen column j/k aim for (Vim's curswant); usize::MAX after `$`.
    want: Option<usize>,
    anchor: Pos,
    pending: Vec<Key>,
    register: Register,
    last_find: Option<(Find, char)>,
    undo: Vec<Group>,
    redo: Vec<Group>,
    group: Option<Group>,
    session: Option<Session>,
    last_change: Option<LastChange>,
    /// Keys of the change being recorded, for `.`, and its count.
    recording: Option<Vec<Key>>,
    recording_count: Option<usize>,
    replaying: bool,
    pub shiftwidth: usize,
    pub tabstop: usize,
}

impl Default for Vim {
    fn default() -> Self {
        Self::new()
    }
}

impl Vim {
    pub fn new() -> Self {
        Self {
            mode: Mode::Normal,
            cursor: 0,
            want: None,
            anchor: 0,
            pending: Vec::new(),
            register: Register::default(),
            last_find: None,
            undo: Vec::new(),
            redo: Vec::new(),
            group: None,
            session: None,
            last_change: None,
            recording: None,
            recording_count: None,
            replaying: false,
            shiftwidth: 8,
            tabstop: 8,
        }
    }

    pub fn mode(&self) -> Mode {
        if !self.pending.is_empty() && self.mode == Mode::Normal && self.pending_operator() {
            Mode::OperatorPending
        } else {
            self.mode
        }
    }

    pub fn cursor(&self) -> Pos {
        self.cursor
    }

    pub fn set_cursor(&mut self, pos: Pos) {
        self.cursor = pos;
        self.want = None;
    }

    /// Where a visual selection started, while one is active.
    pub fn visual_start(&self) -> Option<Pos> {
        matches!(self.mode, Mode::Visual | Mode::VisualLine).then_some(self.anchor)
    }

    pub fn register(&self) -> &Register {
        &self.register
    }

    /// The keys of a command still being typed ("d2", "g"), for showing.
    pub fn pending(&self) -> &[Key] {
        &self.pending
    }

    fn pending_operator(&self) -> bool {
        let mut i = 0;
        while i < self.pending.len()
            && matches!(self.pending[i], Key::Char(c) if c.is_ascii_digit())
        {
            i += 1;
        }
        matches!(
            self.pending.get(i),
            Some(Key::Char('d' | 'c' | 'y' | '<' | '>'))
        ) || matches!(
            (self.pending.get(i), self.pending.get(i + 1)),
            (Some(Key::Char('g')), Some(Key::Char('u' | 'U' | '~')))
        )
    }

    /// Handle one key. On `Err(Beep)` the command failed and Vim would drop
    /// the keys typed after it.
    pub fn key(&mut self, t: &mut dyn TextModel, key: Key) -> R {
        if !self.replaying
            && let Some(rec) = self.recording.as_mut()
        {
            rec.push(key);
        }
        let result = match self.mode {
            Mode::Insert | Mode::Replace => self.insert_key(t, key),
            _ => self.command_key(t, key),
        };
        if result.is_err() {
            self.pending.clear();
            if self.session.is_none() {
                self.recording = None;
                self.close_group();
            }
        }
        result
    }

    // ── Parsing ──────────────────────────────────────────────────────────

    fn command_key(&mut self, t: &mut dyn TextModel, key: Key) -> R {
        self.pending.push(key);
        let visual = matches!(self.mode, Mode::Visual | Mode::VisualLine);
        match parse(&self.pending, visual) {
            Parse::Incomplete => Ok(()),
            Parse::Invalid => Err(Beep),
            Parse::Done(cmd) => {
                let keys = std::mem::take(&mut self.pending);
                self.run(t, cmd, keys)
            }
        }
    }

    // ── Running commands ─────────────────────────────────────────────────

    fn run(&mut self, t: &mut dyn TextModel, cmd: Command, keys: Vec<Key>) -> R {
        let visual = matches!(self.mode, Mode::Visual | Mode::VisualLine);
        let changes = is_change(cmd.action, visual);
        if changes && !self.replaying {
            // Record for `.` without the leading count, which `.` can replace.
            let body: Vec<Key> = keys
                .iter()
                .copied()
                .skip_while(|k| matches!(k, Key::Char(c) if c.is_ascii_digit() && *c != '0'))
                .collect();
            self.recording = Some(if visual { keys.clone() } else { body });
            self.recording_count = cmd.count;
        }
        let visual_extent = visual.then(|| self.visual_extent(t));
        let result = if visual {
            self.run_visual(t, cmd)
        } else {
            self.run_normal(t, cmd)
        };
        if result.is_ok() && changes && !self.replaying && self.session.is_none() {
            self.finish_change(cmd.count, visual_extent);
        }
        if self.session.is_none() {
            self.close_group();
        }
        result
    }

    fn finish_change(&mut self, count: Option<usize>, visual: Option<(bool, usize, usize)>) {
        if let Some(keys) = self.recording.take() {
            self.last_change = Some(match visual {
                Some((linewise, lines, last)) => LastChange::Visual {
                    linewise,
                    lines,
                    last_col_or_chars: last,
                    keys,
                },
                None => LastChange::Keys {
                    count: count.or(self.recording_count),
                    keys,
                },
            });
        }
    }

    fn run_normal(&mut self, t: &mut dyn TextModel, cmd: Command) -> R {
        let count = cmd.count.unwrap_or(1);
        match cmd.action {
            Action::Move(m) => {
                let (to, _) = self.motion(t, m, cmd.count, false)?;
                self.cursor = to;
                self.clamp(t);
                Ok(())
            }
            Action::Operate(op, Some(m)) => self.operate_motion(t, op, m, cmd.count),
            Action::Operate(op, None) => self.operate_lines(t, op, count),
            Action::Insert(kind) => self.start_insert(t, kind, count),
            Action::Replace => {
                self.begin_group();
                self.session = Some(Session::new(Some(Insert::Before), count, self.cursor));
                self.mode = Mode::Replace;
                Ok(())
            }
            Action::ReplaceChar(c) => self.replace_chars(t, c, count),
            Action::Join(spaces) => {
                let (line, _) = self.lc(t);
                let n = count.max(2);
                if line + n - 1 > last_line(t) {
                    if n <= 2 {
                        return Err(Beep);
                    }
                    if line == last_line(t) {
                        // Vim joins the one line with itself: the cursor goes to its start.
                        self.cursor = t.line_to_char(line);
                        self.want = None;
                        return Ok(());
                    }
                }
                let n = n.min(last_line(t) - line + 1);
                self.begin_group();
                self.join(t, line, n, spaces);
                Ok(())
            }
            Action::ToggleCase => {
                let (line, col) = self.lc(t);
                let len = line_len(t, line);
                if len == 0 {
                    return Err(Beep);
                }
                let end = (col + count).min(len);
                let start = self.cursor;
                self.begin_group();
                let stop = text::pos(t, line, end);
                self.map_case(t, start..stop, Op::Toggle);
                self.cursor = text::pos(t, line, end.min(len - 1));
                self.want = None;
                Ok(())
            }
            Action::Put(before) => self.put(t, before, count),
            Action::Visual(linewise) => {
                self.anchor = self.cursor;
                self.mode = if linewise {
                    Mode::VisualLine
                } else {
                    Mode::Visual
                };
                Ok(())
            }
            Action::Undo => self.undo(t, count),
            Action::Redo => self.redo(t, count),
            Action::Repeat => self.repeat(t, cmd.count),
            Action::Cancel => Ok(()),
            Action::SwapEnds => Err(Beep),
        }
    }

    fn lc(&self, t: &dyn TextModel) -> (usize, usize) {
        text::line_col(t, self.cursor)
    }

    fn cur(&self, t: &dyn TextModel) -> Cur {
        let (l, c) = self.lc(t);
        Cur::new(l, c)
    }

    fn at(t: &dyn TextModel, c: Cur) -> Pos {
        text::pos(t, c.line, c.col)
    }

    /// Keep the cursor on a char, as normal mode does.
    fn clamp(&mut self, t: &dyn TextModel) {
        self.cursor = self.cursor.min(t.len_chars());
        let (line, col) = self.lc(t);
        let len = line_len(t, line);
        if col >= len {
            self.cursor = text::pos(t, line, len.saturating_sub(1));
        }
    }

    /// Where a motion goes and what kind it is. `op`: under an operator.
    fn motion(
        &mut self,
        t: &dyn TextModel,
        m: Motion,
        count: Option<usize>,
        op: bool,
    ) -> R<(Pos, Kind)> {
        let n = count.unwrap_or(1);
        let c = self.cur(t);
        let len = line_len(t, c.line);
        let ts = self.tabstop;
        let want = |s: &Self| s.want.unwrap_or_else(|| text::vcol(t, c.line, c.col, ts));
        let (to, kind) = match m {
            Motion::Left => {
                if c.col == 0 {
                    if op {
                        return Ok((self.cursor, Kind::Exclusive));
                    }
                    return Err(Beep);
                }
                self.want = None;
                (Cur::new(c.line, c.col.saturating_sub(n)), Kind::Exclusive)
            }
            Motion::Right => {
                let max = if op { len } else { len.saturating_sub(1) };
                if c.col >= max {
                    if op {
                        // `cl` on an empty line changes nothing and inserts.
                        return Ok((self.cursor, Kind::Exclusive));
                    }
                    return Err(Beep);
                }
                self.want = None;
                (Cur::new(c.line, (c.col + n).min(max)), Kind::Exclusive)
            }
            Motion::Down | Motion::Up => {
                let down = m == Motion::Down;
                let target = if down {
                    c.line + n
                } else {
                    c.line.wrapping_sub(n)
                };
                if (down && c.line == last_line(t)) || (!down && c.line == 0) {
                    return Err(Beep);
                }
                let target = if down {
                    target.min(last_line(t))
                } else if n > c.line {
                    0
                } else {
                    target
                };
                let v = want(self);
                self.want = Some(v);
                (
                    Cur::new(target, text::col_at_vcol(t, target, v, ts)),
                    Kind::Linewise,
                )
            }
            Motion::LineStart => {
                self.want = None;
                (Cur::new(c.line, 0), Kind::Exclusive)
            }
            Motion::FirstNonBlank => {
                self.want = None;
                (
                    Cur::new(
                        c.line,
                        first_non_blank(t, c.line).min(len.saturating_sub(1)),
                    ),
                    Kind::Exclusive,
                )
            }
            Motion::LineEnd => {
                let line = c.line + n - 1;
                if line > last_line(t) {
                    return Err(Beep);
                }
                self.want = Some(usize::MAX);
                let len = line_len(t, line);
                (Cur::new(line, len.saturating_sub(1)), Kind::Inclusive)
            }
            Motion::LastNonBlank => {
                let line = c.line + n - 1;
                if line > last_line(t) {
                    return Err(Beep);
                }
                self.want = None;
                let s = line_text(t, line);
                let last = s
                    .chars()
                    .collect::<Vec<_>>()
                    .iter()
                    .rposition(|&ch| !is_blank(ch))
                    .unwrap_or(0);
                (Cur::new(line, last), Kind::Inclusive)
            }
            Motion::Column => {
                self.want = None;
                let col = text::col_at_vcol(t, c.line, n - 1, ts);
                (
                    Cur::new(c.line, col.min(len.saturating_sub(1))),
                    Kind::Exclusive,
                )
            }
            Motion::GotoFirst | Motion::GotoLast => {
                let line = match (m, count) {
                    (_, Some(n)) => (n - 1).min(last_line(t)),
                    (Motion::GotoFirst, None) => 0,
                    _ => last_line(t),
                };
                let v = want(self);
                self.want = Some(v);
                (
                    Cur::new(line, text::col_at_vcol(t, line, v, ts)),
                    Kind::Linewise,
                )
            }
            Motion::NextLine | Motion::PrevLine | Motion::CurrentLine => {
                let line = match m {
                    Motion::NextLine => {
                        if c.line + n > last_line(t) {
                            return Err(Beep);
                        }
                        c.line + n
                    }
                    Motion::PrevLine => c.line.checked_sub(n).ok_or(Beep)?,
                    _ => (c.line + n - 1).min(last_line(t)),
                };
                self.want = None;
                (Cur::new(line, first_non_blank(t, line)), Kind::Linewise)
            }
            Motion::Word(big) => {
                let mut start = c;
                // `cw` on a word is `ce`, staying on this word's end.
                self.want = None;
                let (to, _ok) = motion::fwd_word(t, start, n, big, op);
                start = to;
                (start, Kind::Exclusive)
            }
            Motion::WordEnd(big) => {
                self.want = None;
                let (to, _ok) = motion::end_word(t, c, n, big, false, false);
                if to == c {
                    return Err(Beep);
                }
                (to, Kind::Inclusive)
            }
            Motion::WordBack(big) => {
                self.want = None;
                let (to, ok) = motion::bck_word(t, c, n, big, false);
                if !ok {
                    return Err(Beep);
                }
                (to, Kind::Exclusive)
            }
            Motion::WordEndBack(big) => {
                self.want = None;
                let (to, ok) = motion::bckend_word(t, c, n, big, false);
                if !ok {
                    return Err(Beep);
                }
                (to, Kind::Inclusive)
            }
            Motion::Find(kind, ch) => {
                self.last_find = Some((kind, ch));
                self.want = None;
                let to = motion::find_char(t, c, kind, ch, n, false).ok_or(Beep)?;
                (
                    to,
                    if kind.forward() {
                        Kind::Inclusive
                    } else {
                        Kind::Exclusive
                    },
                )
            }
            Motion::RepeatFind(reverse) => {
                let (kind, ch) = self.last_find.ok_or(Beep)?;
                let kind = if reverse { kind.reversed() } else { kind };
                self.want = None;
                let to = motion::find_char(t, c, kind, ch, n, true).ok_or(Beep)?;
                (
                    to,
                    if kind.forward() {
                        Kind::Inclusive
                    } else {
                        Kind::Exclusive
                    },
                )
            }
            Motion::ParagraphForward | Motion::ParagraphBack => {
                let (to, inclusive) =
                    motion::paragraph(t, c.line, n, m == Motion::ParagraphForward).ok_or(Beep)?;
                self.want = None;
                (
                    to,
                    if inclusive {
                        Kind::Inclusive
                    } else {
                        Kind::Exclusive
                    },
                )
            }
            Motion::Match => {
                // No bracket on the line: Neovim stays put, no error.
                let to = motion::match_pair(t, c).unwrap_or(c);
                self.want = None;
                (to, Kind::Inclusive)
            }
        };
        Ok((Self::at(t, to), kind))
    }

    // ── Operators ────────────────────────────────────────────────────────

    fn operate_motion(
        &mut self,
        t: &mut dyn TextModel,
        op: Op,
        m: Motion,
        count: Option<usize>,
    ) -> R {
        let start_cur = self.cur(t);
        let n = count.unwrap_or(1);
        let (mut to, mut kind) = match m {
            // `cw`/`cW` on a word changes to its end (Vim's `cw` = `ce` rule).
            Motion::Word(big)
                if op == Op::Change
                    && char_at(t, start_cur.line, start_cur.col)
                        .is_some_and(|ch| !is_blank(ch)) =>
            {
                let (to, _) = motion::end_word(t, start_cur, n, big, true, false);
                self.want = None;
                (Self::at(t, to), Kind::Inclusive)
            }
            _ => self.motion(t, m, count, true)?,
        };
        // `w` landing past the end of a line comes back onto its last char
        // and becomes inclusive (Vim's adjust_cursor).
        if matches!(m, Motion::Word(_)) {
            let (l, c) = text::line_col(t, to);
            if c > 0 && c == line_len(t, l) && to > self.cursor && kind == Kind::Exclusive {
                to -= 1;
                kind = Kind::Inclusive;
            }
        }
        let (mut start, mut end) = if to < self.cursor {
            (to, self.cursor)
        } else {
            (self.cursor, to)
        };
        let (sl, _) = text::line_col(t, start);
        let (mut el, ec) = text::line_col(t, end);

        // An exclusive motion ending in column 0 of a later line ends at the end
        // of the line before, or becomes linewise (Vim's `:help exclusive`).
        if kind == Kind::Exclusive && ec == 0 && el > sl {
            el -= 1;
            let start_c = text::line_col(t, start).1;
            if start_c <= first_non_blank(t, sl) {
                kind = Kind::Linewise;
            } else {
                let len = line_len(t, el);
                end = text::pos(t, el, len);
                if len > 0 {
                    end -= 1;
                    kind = Kind::Inclusive;
                }
            }
        }
        // `d` over several lines with only blanks around becomes linewise.
        if op == Op::Delete && kind != Kind::Linewise && el > sl {
            let after = if kind == Kind::Inclusive {
                end + 1
            } else {
                end
            };
            let (_, ac) = text::line_col(t, after.min(text::pos(t, el, line_len(t, el))));
            let rest: String = line_text(t, el).chars().skip(ac).collect();
            let start_c = text::line_col(t, start).1;
            if rest.chars().all(is_blank) && start_c <= first_non_blank(t, sl) {
                kind = Kind::Linewise;
            }
        }
        if kind == Kind::Linewise {
            let v = match self.want {
                // After `$` a linewise delete keeps the cursor's own column.
                Some(w) if w != usize::MAX || op != Op::Delete => w,
                _ => text::vcol(t, start_cur.line, start_cur.col, self.tabstop),
            };
            return self.apply_lines(t, op, sl, el, v);
        }
        if kind == Kind::Inclusive {
            let (l, c) = text::line_col(t, end);
            // At a line's end nothing is taken, except by `%`: Neovim's `%` is
            // the matchit plugin, which selects visually, taking the line
            // break of an empty line.
            if c < line_len(t, l) || (m == Motion::Match && l == sl && l < last_line(t)) {
                end += 1;
            }
        }
        start = start.min(t.len_chars());
        end = end.min(t.len_chars());
        self.apply_chars(t, op, start..end)
    }

    fn operate_lines(&mut self, t: &mut dyn TextModel, op: Op, count: usize) -> R {
        let (line, col) = self.lc(t);
        if count > 1 && line == last_line(t) {
            return Err(Beep);
        }
        let count = count.min(last_line(t) - line + 1);
        let v = match op {
            // Vim moves to the first non-blank before these (nv_lineop).
            Op::Change | Op::Lower | Op::Upper | Op::Toggle => {
                text::vcol(t, line, first_non_blank(t, line), self.tabstop)
            }
            _ => self
                .want
                .unwrap_or_else(|| text::vcol(t, line, col, self.tabstop)),
        };
        self.apply_lines(t, op, line, line + count - 1, v)
    }

    /// An operator over whole lines `first..=last`. `vcol`: the screen column
    /// the cursor keeps afterwards (Neovim's 'nostartofline').
    fn apply_lines(
        &mut self,
        t: &mut dyn TextModel,
        op: Op,
        first: usize,
        last: usize,
        vcol: usize,
    ) -> R {
        let start = t.line_to_char(first);
        let end = text::pos(t, last, line_len(t, last));
        let mut yanked = t.slice(start..end);
        yanked.push('\n');
        let ts = self.tabstop;
        let place = |s: &mut Self, t: &dyn TextModel, line: usize| {
            let col = text::col_at_vcol(t, line, vcol, ts);
            s.cursor = text::pos(t, line, col.min(line_len(t, line).saturating_sub(1)));
        };
        match op {
            Op::Yank => {
                self.register = Register {
                    text: yanked,
                    linewise: true,
                };
                let (line, _) = self.lc(t);
                if first < line {
                    place(self, t, first);
                }
                Ok(())
            }
            Op::Delete => {
                self.register = Register {
                    text: yanked,
                    linewise: true,
                };
                self.begin_group();
                if last == last_line(t) && first > 0 {
                    // The last lines go with the line break before them.
                    let from = text::pos(t, first - 1, line_len(t, first - 1));
                    self.edit(t, from..end, "");
                    if let Some(e) = self.group.as_mut().and_then(|g| g.edits.last_mut()) {
                        e.line = first;
                    }
                    place(self, t, first - 1);
                } else if last == last_line(t) {
                    self.edit(t, 0..end, "");
                    self.cursor = 0;
                } else {
                    let to = t.line_to_char(last + 1);
                    self.edit(t, start..to, "");
                    place(self, t, first);
                }
                Ok(())
            }
            Op::Change => {
                self.register = Register {
                    text: yanked,
                    linewise: true,
                };
                self.begin_group();
                let ind = indent(t, first);
                let from = t.line_to_char(first);
                self.edit(t, from..end, &ind);
                self.cursor = from + ind.chars().count();
                let mut s = Session::new(None, 1, self.cursor);
                s.did_ai = !ind.is_empty();
                self.session = Some(s);
                self.mode = Mode::Insert;
                Ok(())
            }
            Op::ShiftRight | Op::ShiftLeft => {
                self.begin_group();
                for line in first..=last {
                    self.shift_line(t, line, op == Op::ShiftRight);
                }
                place(self, t, first);
                Ok(())
            }
            Op::Lower | Op::Upper | Op::Toggle => {
                self.begin_group();
                self.map_case(t, start..end, op);
                place(self, t, first);
                Ok(())
            }
        }
    }

    /// An operator over the chars in `range`.
    fn apply_chars(&mut self, t: &mut dyn TextModel, op: Op, range: std::ops::Range<Pos>) -> R {
        let yanked = t.slice(range.clone());
        match op {
            Op::Yank => {
                self.register = Register {
                    text: yanked,
                    linewise: false,
                };
                self.cursor = range.start;
                self.clamp(t);
                Ok(())
            }
            Op::Delete => {
                if range.is_empty() {
                    return Ok(());
                }
                self.register = Register {
                    text: yanked,
                    linewise: false,
                };
                self.begin_group();
                self.edit(t, range.clone(), "");
                self.cursor = range.start;
                self.clamp(t);
                self.want = None;
                Ok(())
            }
            Op::Change => {
                if !range.is_empty() {
                    self.register = Register {
                        text: yanked,
                        linewise: false,
                    };
                }
                self.begin_group();
                self.edit(t, range.clone(), "");
                self.cursor = range.start;
                self.session = Some(Session::new(None, 1, self.cursor));
                self.mode = Mode::Insert;
                Ok(())
            }
            Op::ShiftRight | Op::ShiftLeft => {
                let (first, _) = text::line_col(t, range.start);
                let (last, _) = text::line_col(t, range.end.saturating_sub(1).max(range.start));
                let (l, c) = self.lc(t);
                let v = self
                    .want
                    .unwrap_or_else(|| text::vcol(t, l, c, self.tabstop));
                self.apply_lines(t, op, first, last, v)
            }
            Op::Lower | Op::Upper | Op::Toggle => {
                self.begin_group();
                self.map_case(t, range.clone(), op);
                self.cursor = range.start;
                self.clamp(t);
                Ok(())
            }
        }
    }

    fn shift_line(&mut self, t: &mut dyn TextModel, line: usize, right: bool) {
        let len = line_len(t, line);
        if len == 0 {
            return;
        }
        let ind = indent(t, line);
        let width: usize = ind.chars().fold(0, |w, c| {
            if c == '\t' {
                (w / self.tabstop + 1) * self.tabstop
            } else {
                w + 1
            }
        });
        let new = if right {
            width + self.shiftwidth
        } else {
            width.saturating_sub(self.shiftwidth)
        };
        let mut s = "\t".repeat(new / self.tabstop);
        s.push_str(&" ".repeat(new % self.tabstop));
        if s != ind {
            let start = t.line_to_char(line);
            self.edit(t, start..start + ind.chars().count(), &s);
        }
    }

    fn map_case(&mut self, t: &mut dyn TextModel, range: std::ops::Range<Pos>, op: Op) {
        let old = t.slice(range.clone());
        let new: String = old
            .chars()
            .map(|c| {
                let one = |mut it: Box<dyn Iterator<Item = char>>| {
                    let first = it.next().unwrap_or(c);
                    if it.next().is_some() { c } else { first }
                };
                match op {
                    Op::Lower => one(Box::new(c.to_lowercase())),
                    Op::Upper => one(Box::new(c.to_uppercase())),
                    _ if c.is_uppercase() => one(Box::new(c.to_lowercase())),
                    _ if c.is_lowercase() => one(Box::new(c.to_uppercase())),
                    _ => c,
                }
            })
            .collect();
        if new != old {
            self.edit(t, range, &new);
        }
    }

    fn replace_chars(&mut self, t: &mut dyn TextModel, c: char, count: usize) -> R {
        let (line, col) = self.lc(t);
        if col + count > line_len(t, line) {
            return Err(Beep);
        }
        self.begin_group();
        let start = self.cursor;
        self.edit(t, start..start + count, &c.to_string().repeat(count));
        self.cursor = start + count - 1;
        self.want = None;
        Ok(())
    }

    /// Join `n` lines from `line` (Vim's do_join).
    fn join(&mut self, t: &mut dyn TextModel, line: usize, n: usize, spaces: bool) {
        let mut join_col = 0;
        for _ in 1..n {
            let cur_len = line_len(t, line);
            let cur = line_text(t, line);
            let next = line_text(t, line + 1);
            let lead = if spaces {
                next.chars().take_while(|&c| is_blank(c)).count()
            } else {
                0
            };
            let rest: String = next.chars().skip(lead).collect();
            let last = cur.chars().last();
            let space = spaces
                && !rest.is_empty()
                && !rest.starts_with(')')
                && cur_len > 0
                && last != Some('\t')
                && last != Some(' ');
            let from = text::pos(t, line, cur_len);
            let to = t.line_to_char(line + 1) + lead;
            self.edit(t, from..to, if space { " " } else { "" });
            join_col = cur_len;
        }
        self.cursor = text::pos(t, line, join_col);
        self.clamp(t);
        self.want = None;
    }

    fn put(&mut self, t: &mut dyn TextModel, before: bool, count: usize) -> R {
        let reg = self.register.clone();
        if reg.text.is_empty() {
            return Err(Beep);
        }
        self.begin_group();
        let (line, col) = self.lc(t);
        if reg.linewise {
            let body = reg.text.strip_suffix('\n').unwrap_or(&reg.text);
            let block = vec![body; count].join("\n");
            let target = if before {
                let at = t.line_to_char(line);
                self.edit(t, at..at, &format!("{block}\n"));
                line
            } else {
                let at = text::pos(t, line, line_len(t, line));
                self.edit(t, at..at, &format!("\n{block}"));
                line + 1
            };
            self.cursor = text::pos(
                t,
                target,
                first_non_blank(t, target).min(line_len(t, target).saturating_sub(1)),
            );
        } else {
            let block = reg.text.repeat(count);
            let at = if before || line_len(t, line) == 0 {
                self.cursor
            } else {
                text::pos(t, line, col + 1)
            };
            self.edit(t, at..at, &block);
            if block.contains('\n') {
                self.cursor = at;
            } else {
                self.cursor = at + block.chars().count() - 1;
            }
        }
        self.want = None;
        Ok(())
    }

    // ── Insert and replace ───────────────────────────────────────────────

    fn start_insert(&mut self, t: &mut dyn TextModel, kind: Insert, count: usize) -> R {
        self.begin_group();
        let (line, col) = self.lc(t);
        let len = line_len(t, line);
        let mut did_ai = false;
        match kind {
            Insert::Before => {}
            Insert::After => {
                if len > 0 {
                    self.cursor = text::pos(t, line, col + 1);
                }
            }
            Insert::LineStart => self.cursor = text::pos(t, line, first_non_blank(t, line)),
            Insert::LineEnd => self.cursor = text::pos(t, line, len),
            Insert::Below | Insert::Above => {
                let ind = indent(t, line);
                if kind == Insert::Below {
                    let at = text::pos(t, line, len);
                    self.edit(t, at..at, &format!("\n{ind}"));
                    self.cursor = text::pos(t, line + 1, ind.chars().count());
                } else {
                    let at = t.line_to_char(line);
                    self.edit(t, at..at, &format!("{ind}\n"));
                    self.cursor = text::pos(t, line, ind.chars().count());
                }
                did_ai = !ind.is_empty();
            }
        }
        let mut s = Session::new(Some(kind), count, self.cursor);
        s.did_ai = did_ai;
        self.session = Some(s);
        self.mode = Mode::Insert;
        Ok(())
    }

    fn insert_key(&mut self, t: &mut dyn TextModel, key: Key) -> R {
        let replace = self.mode == Mode::Replace;
        if key != Key::Esc
            && let Some(s) = self.session.as_mut()
        {
            s.typed.push(key);
        }
        match key {
            Key::Esc => {
                self.leave_insert(t);
                Ok(())
            }
            Key::Char(c) => {
                self.type_char(t, c, replace);
                Ok(())
            }
            Key::Tab => {
                self.type_char(t, '\t', replace);
                Ok(())
            }
            Key::Enter => {
                let (line, col) = self.lc(t);
                let ind: String = indent(t, line).chars().take(col).collect();
                self.remove_lone_autoindent(t);
                let at = self.cursor;
                // With autoindent, blanks that would start the new line go.
                let (l, c) = self.lc(t);
                let rest = line_text(t, l)
                    .chars()
                    .skip(c)
                    .take_while(|&ch| is_blank(ch))
                    .count();
                self.edit(t, at..at + rest, &format!("\n{ind}"));
                self.cursor = at + 1 + ind.chars().count();
                if let Some(s) = self.session.as_mut() {
                    s.did_ai = !ind.is_empty();
                    if replace {
                        s.replaced.push(None);
                    }
                }
                Ok(())
            }
            Key::Backspace | Key::Ctrl('h') => {
                if replace {
                    self.replace_backspace(t);
                    return Ok(());
                }
                if self.cursor == 0 {
                    return Ok(());
                }
                let at = self.cursor;
                let to = self.backspace_target(t);
                self.edit(t, to..at, "");
                self.cursor = to;
                self.clear_ai();
                Ok(())
            }
            Key::Ctrl('w') | Key::Ctrl('u') => {
                self.delete_back(t, key == Key::Ctrl('w'));
                Ok(())
            }
            Key::Delete => {
                if self.cursor < t.len_chars() {
                    let at = self.cursor;
                    self.edit(t, at..at + 1, "");
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn clear_ai(&mut self) {
        if let Some(s) = self.session.as_mut() {
            s.did_ai = false;
        }
    }

    /// The line has only its autoindent: remove it (Esc or Enter on it).
    fn remove_lone_autoindent(&mut self, t: &mut dyn TextModel) {
        if self.session.as_ref().is_some_and(|s| s.did_ai) {
            let (line, _) = self.lc(t);
            let ind = indent(t, line);
            if ind.chars().count() == line_len(t, line) && !ind.is_empty() {
                let start = t.line_to_char(line);
                self.edit(t, start..start + ind.chars().count(), "");
                self.cursor = start;
            }
            self.clear_ai();
        }
    }

    fn type_char(&mut self, t: &mut dyn TextModel, c: char, replace: bool) {
        let at = self.cursor;
        let (line, col) = self.lc(t);
        if replace {
            let old = char_at(t, line, col);
            let end = if old.is_some() { at + 1 } else { at };
            self.edit(t, at..end, &c.to_string());
            if let Some(s) = self.session.as_mut() {
                s.replaced.push(old);
            }
        } else {
            self.edit(t, at..at, &c.to_string());
        }
        self.cursor += 1;
        self.clear_ai();
    }

    /// Where Backspace goes back to: one char, or in leading whitespace back
    /// to the previous 'shiftwidth' stop (Neovim's default 'smarttab').
    fn backspace_target(&self, t: &dyn TextModel) -> Pos {
        let (line, col) = self.lc(t);
        if col == 0 {
            return self.cursor - 1;
        }
        let chars: Vec<char> = line_text(t, line).chars().collect();
        if !chars[..col].iter().all(|&c| is_blank(c)) {
            return self.cursor - 1;
        }
        let v = text::vcol(t, line, col, self.tabstop);
        let target = (v - 1) / self.shiftwidth * self.shiftwidth;
        let mut c = col;
        while c > 0 && text::vcol(t, line, c - 1, self.tabstop) >= target {
            c -= 1;
        }
        text::pos(t, line, c)
    }

    fn replace_backspace(&mut self, t: &mut dyn TextModel) {
        let (line, col) = self.lc(t);
        if col == 0 {
            // Back onto the end of the line before (Vim's backspace=eol).
            if line > 0 {
                self.cursor = text::pos(t, line - 1, line_len(t, line - 1));
            }
            return;
        }
        if self.session.as_ref().is_some_and(|s| s.replaced.is_empty()) {
            // Before where replacing began: just move, a shiftwidth in indent.
            self.cursor = self.backspace_target(t);
            return;
        }
        let Some(s) = self.session.as_mut() else {
            return;
        };
        match s.replaced.pop() {
            Some(old) => {
                let at = self.cursor - 1;
                match old {
                    Some(ch) => self.edit(t, at..at + 1, &ch.to_string()),
                    None => {
                        let (line, col) = self.lc(t);
                        if col == 0 && line > 0 {
                            // Undo a line break typed in replace mode.
                            let from = text::pos(t, line - 1, line_len(t, line - 1));
                            let ind = indent(t, line).chars().count();
                            self.edit(t, from..self.cursor.max(from + 1 + ind), "");
                            self.cursor = from + 1;
                        } else {
                            self.edit(t, at..at + 1, "");
                        }
                    }
                }
                self.cursor -= 1;
            }
            None => {
                // Before where replacing started: just move left.
                let (_, col) = self.lc(t);
                if col > 0 {
                    self.cursor -= 1;
                }
            }
        }
    }

    /// Ctrl-w (a word) or Ctrl-u (to the line's start) before the cursor,
    /// stopping once where the insert started.
    fn delete_back(&mut self, t: &mut dyn TextModel, word: bool) {
        let (line, col) = self.lc(t);
        if col == 0 {
            if line > 0 {
                let at = self.cursor;
                self.edit(t, at - 1..at, "");
                self.cursor -= 1;
            }
            return;
        }
        let start = self.session.as_ref().map_or(0, |s| s.start);
        let line_start = t.line_to_char(line);
        let chars: Vec<char> = line_text(t, line).chars().collect();
        let mut c = col;
        if word {
            while c > 0 && is_blank(chars[c - 1]) {
                c -= 1;
            }
            if c > 0 {
                let cl = text::class(Some(chars[c - 1]), false);
                while c > 0 && text::class(Some(chars[c - 1]), false) == cl {
                    c -= 1;
                }
            }
        } else {
            let fnb = chars.iter().take_while(|&&ch| is_blank(ch)).count();
            c = if col > fnb { fnb } else { 0 };
        }
        let mut to = line_start + c;
        if start > line_start && start < self.cursor && to < start {
            to = start;
        }
        let at = self.cursor;
        self.edit(t, to..at, "");
        self.cursor = to;
        if let Some(s) = self.session.as_mut()
            && s.start > to
        {
            s.start = to;
        }
    }

    fn leave_insert(&mut self, t: &mut dyn TextModel) {
        let session = self.session.take();
        if let Some(s) = &session
            && s.count > 1
        {
            let typed: Vec<Key> = s.typed.clone();
            let mode = self.mode;
            self.session = Some(Session::new(s.kind, 1, self.cursor));
            let replaying = std::mem::replace(&mut self.replaying, true);
            for _ in 1..s.count {
                if matches!(s.kind, Some(Insert::Below | Insert::Above)) && mode == Mode::Insert {
                    let (line, _) = self.lc(t);
                    let ind = indent(t, line);
                    let at = text::pos(t, line, line_len(t, line));
                    self.edit(t, at..at, &format!("\n{ind}"));
                    self.cursor = text::pos(t, line + 1, ind.chars().count());
                }
                for &k in &typed {
                    let _ = self.insert_key(t, k);
                }
            }
            self.replaying = replaying;
            self.session = Some(Session {
                did_ai: self.session.as_ref().is_some_and(|x| x.did_ai),
                ..s.clone()
            });
        } else {
            self.session = session;
        }
        self.remove_lone_autoindent(t);
        self.session = None;
        self.mode = Mode::Normal;
        let (_, col) = self.lc(t);
        if col > 0 {
            self.cursor -= 1;
        }
        self.clamp(t);
        self.want = None;
        self.close_group();
        if !self.replaying {
            self.finish_change(None, None);
        }
    }

    // ── Visual mode ──────────────────────────────────────────────────────

    /// (linewise, lines, screen columns) for `.`: on one line the width of
    /// the selection, over several the end's screen column (as Vim's redo).
    fn visual_extent(&self, t: &dyn TextModel) -> (bool, usize, usize) {
        let (a, b) = (self.anchor.min(self.cursor), self.anchor.max(self.cursor));
        let (al, ac) = text::line_col(t, a);
        let (bl, bc) = text::line_col(t, b);
        let linewise = self.mode == Mode::VisualLine;
        let ts = self.tabstop;
        if al == bl {
            (
                linewise,
                1,
                text::vcol(t, bl, bc, ts) - text::vcol(t, al, ac, ts) + 1,
            )
        } else {
            (linewise, bl - al + 1, text::vcol(t, bl, bc, ts))
        }
    }

    fn run_visual(&mut self, t: &mut dyn TextModel, cmd: Command) -> R {
        let linewise = self.mode == Mode::VisualLine;
        let (a, b) = (self.anchor.min(self.cursor), self.anchor.max(self.cursor));
        let (al, _) = text::line_col(t, a);
        let (bl, _) = text::line_col(t, b);
        let count = cmd.count.unwrap_or(1);
        let exit = |s: &mut Self| s.mode = Mode::Normal;
        let result = self.run_visual_inner(t, cmd, a, b, al, bl, linewise, count, exit);
        if !matches!(
            self.mode,
            Mode::Visual | Mode::VisualLine | Mode::Insert | Mode::Replace
        ) {
            self.clamp(t);
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn run_visual_inner(
        &mut self,
        t: &mut dyn TextModel,
        cmd: Command,
        a: Pos,
        b: Pos,
        al: usize,
        bl: usize,
        linewise: bool,
        count: usize,
        exit: fn(&mut Self),
    ) -> R {
        match cmd.action {
            Action::Move(m) => {
                // In visual mode the cursor can rest on a line's end (after
                // j/k from a longer line), selecting its line break.
                let (to, _) = self.motion(t, m, cmd.count, false)?;
                self.cursor = to;
                Ok(())
            }
            Action::Operate(op, None) | Action::Operate(op, Some(_)) => {
                let lines = linewise || matches!(cmd.action, Action::Operate(_, None));
                let keep = self.want.unwrap_or_else(|| {
                    let (l, c) = text::line_col(t, self.cursor);
                    text::vcol(t, l, c, self.tabstop)
                });
                exit(self);
                self.cursor = a;
                if lines {
                    self.apply_lines(t, op, al, bl, keep)?;
                } else {
                    // After `$` the selection includes the line break.
                    let eol = self.want == Some(usize::MAX) && self.cursor >= self.anchor;
                    let end = if eol {
                        text::pos(t, bl, line_len(t, bl)) + 1
                    } else {
                        b + 1
                    }
                    .min(t.len_chars());
                    if op == Op::Yank
                        || op == Op::Change
                        || op == Op::Delete
                        || matches!(op, Op::Lower | Op::Upper | Op::Toggle)
                    {
                        self.apply_chars(t, op, a..end)?;
                    } else {
                        self.apply_lines(t, op, al, bl, keep)?;
                    }
                }
                if op == Op::Yank {
                    // A visual yank leaves the cursor at the selection's start.
                    self.cursor = a;
                }
                Ok(())
            }
            Action::Join(spaces) => {
                exit(self);
                let n = (bl - al + 1).max(2);
                if al + n - 1 > last_line(t) {
                    return Err(Beep);
                }
                self.begin_group();
                self.join(t, al, n, spaces);
                Ok(())
            }
            Action::ReplaceChar(c) => {
                exit(self);
                self.begin_group();
                let (start, end) = if linewise {
                    (t.line_to_char(al), text::pos(t, bl, line_len(t, bl)))
                } else {
                    (a, (b + 1).min(t.len_chars()))
                };
                let old = t.slice(start..end);
                let new: String = old
                    .chars()
                    .map(|ch| if ch == '\n' { '\n' } else { c })
                    .collect();
                self.edit(t, start..end, &new);
                self.cursor = start;
                self.clamp(t);
                Ok(())
            }
            Action::ToggleCase => {
                exit(self);
                self.cursor = a;
                let end = (b + 1).min(t.len_chars());
                if linewise {
                    self.apply_lines(t, Op::Toggle, al, bl, 0)
                } else {
                    self.apply_chars(t, Op::Toggle, a..end)
                }
            }
            Action::Put(_) => {
                let reg = self.register.clone();
                exit(self);
                self.begin_group();
                let (start, end) = if linewise {
                    (t.line_to_char(al), text::pos(t, bl, line_len(t, bl)))
                } else {
                    (a, (b + 1).min(t.len_chars()))
                };
                let old = t.slice(start..end);
                let put = if reg.linewise && !linewise {
                    format!("\n{}", reg.text)
                } else if !reg.linewise && linewise {
                    reg.text.clone()
                } else {
                    reg.text
                        .strip_suffix('\n')
                        .map(str::to_string)
                        .unwrap_or(reg.text.clone())
                };
                self.edit(t, start..end, &put.repeat(count));
                self.register = Register {
                    text: if linewise { format!("{old}\n") } else { old },
                    linewise,
                };
                if reg.linewise {
                    let (sl, _) = text::line_col(t, start);
                    let line = if linewise { sl } else { sl + 1 };
                    self.cursor = text::pos(
                        t,
                        line,
                        first_non_blank(t, line).min(line_len(t, line).saturating_sub(1)),
                    );
                } else if put.contains('\n') {
                    self.cursor = start;
                } else {
                    self.cursor = start + put.chars().count().saturating_sub(1);
                }
                self.clamp(t);
                Ok(())
            }
            Action::Visual(to_lines) => {
                if to_lines == linewise {
                    exit(self);
                } else {
                    self.mode = if to_lines {
                        Mode::VisualLine
                    } else {
                        Mode::Visual
                    };
                }
                Ok(())
            }
            Action::SwapEnds => {
                std::mem::swap(&mut self.anchor, &mut self.cursor);
                self.want = None;
                Ok(())
            }
            Action::Cancel => {
                exit(self);
                Ok(())
            }
            Action::Insert(_) | Action::Replace | Action::Undo | Action::Redo | Action::Repeat => {
                exit(self);
                Err(Beep)
            }
        }
    }

    // ── Undo and repeat ──────────────────────────────────────────────────

    fn begin_group(&mut self) {
        if self.group.is_none() {
            self.group = Some(Group {
                edits: Vec::new(),
                cursor_before: self.cursor,
            });
        }
    }

    fn close_group(&mut self) {
        if let Some(g) = self.group.take()
            && !g.edits.is_empty()
        {
            self.undo.push(g);
            self.redo.clear();
        }
    }

    /// Every change to the text goes through here.
    fn edit(&mut self, t: &mut dyn TextModel, range: std::ops::Range<Pos>, with: &str) {
        let removed = t.slice(range.clone());
        if removed == with {
            return;
        }
        let line = t.char_to_line(range.start.min(t.len_chars()));
        t.replace(range.clone(), with);
        self.begin_group();
        if let Some(g) = self.group.as_mut() {
            g.edits.push(Edit {
                at: range.start,
                removed,
                inserted: with.to_string(),
                line,
            });
        }
    }

    fn undo(&mut self, t: &mut dyn TextModel, count: usize) -> R {
        for i in 0..count {
            let Some(g) = self.undo.pop() else {
                // "Already at oldest change": a beep, unless some were undone.
                return if i == 0 { Err(Beep) } else { Ok(()) };
            };
            for e in g.edits.iter().rev() {
                t.replace(e.at..e.at + e.inserted.chars().count(), &e.removed);
            }
            let line = g
                .edits
                .iter()
                .map(|e| e.line)
                .min()
                .unwrap_or(0)
                .min(last_line(t));
            let (bl, bc) = text::line_col(t, g.cursor_before.min(t.len_chars()));
            self.cursor = if bl == line {
                text::pos(t, line, bc)
            } else {
                t.line_to_char(line)
            };
            self.redo.push(g);
        }
        self.clamp(t);
        self.want = None;
        Ok(())
    }

    fn redo(&mut self, t: &mut dyn TextModel, count: usize) -> R {
        for _ in 0..count {
            let Some(g) = self.redo.pop() else {
                return Ok(());
            };
            for e in &g.edits {
                t.replace(e.at..e.at + e.removed.chars().count(), &e.inserted);
            }
            let line = g
                .edits
                .iter()
                .map(|e| e.line)
                .min()
                .unwrap_or(0)
                .min(last_line(t));
            let (bl, bc) = text::line_col(t, g.cursor_before.min(t.len_chars()));
            self.cursor = if bl == line {
                text::pos(t, line, bc)
            } else {
                t.line_to_char(line)
            };
            self.undo.push(g);
        }
        self.clamp(t);
        self.want = None;
        Ok(())
    }

    fn repeat(&mut self, t: &mut dyn TextModel, count: Option<usize>) -> R {
        let Some(last) = self.last_change.clone() else {
            return Ok(());
        };
        let replaying = std::mem::replace(&mut self.replaying, true);
        let result = (|| -> R {
            match &last {
                LastChange::Keys { count: orig, keys } => {
                    let n = count.or(*orig);
                    let mut all: Vec<Key> = n
                        .map(|n| n.to_string().chars().map(Key::Char).collect())
                        .unwrap_or_default();
                    all.extend(keys.iter().copied());
                    for k in all {
                        self.key(t, k)?;
                    }
                }
                LastChange::Visual {
                    linewise,
                    lines,
                    last_col_or_chars,
                    keys,
                } => {
                    let (line, col) = self.lc(t);
                    self.anchor = self.cursor;
                    let end_line = (line + lines - 1).min(last_line(t));
                    let ts = self.tabstop;
                    self.cursor = if *lines == 1 && !linewise {
                        let v = text::vcol(t, line, col, ts) + last_col_or_chars - 1;
                        text::pos(
                            t,
                            line,
                            text::col_at_vcol(t, line, v, ts)
                                .min(line_len(t, line).saturating_sub(1)),
                        )
                    } else {
                        text::pos(
                            t,
                            end_line,
                            text::col_at_vcol(t, end_line, *last_col_or_chars, ts),
                        )
                    };
                    self.mode = if *linewise {
                        Mode::VisualLine
                    } else {
                        Mode::Visual
                    };
                    // The operator is the keys' last command.
                    let op: Vec<Key> = keys
                        .iter()
                        .copied()
                        .skip_while(|k| !is_operator_key(*k))
                        .collect();
                    for k in op {
                        self.key(t, k)?;
                    }
                }
            }
            Ok(())
        })();
        self.replaying = replaying;
        if let (Some(n), LastChange::Keys { keys, .. }) = (count, &last) {
            self.last_change = Some(LastChange::Keys {
                count: Some(n),
                keys: keys.clone(),
            });
        }
        result
    }
}

impl Session {
    fn new(kind: Option<Insert>, count: usize, start: Pos) -> Self {
        Self {
            kind,
            count,
            typed: Vec::new(),
            start,
            did_ai: false,
            replaced: Vec::new(),
        }
    }
}

fn is_operator_key(k: Key) -> bool {
    matches!(
        k,
        Key::Char(
            'd' | 'x'
                | 'c'
                | 's'
                | 'y'
                | '>'
                | '<'
                | '~'
                | 'u'
                | 'U'
                | 'J'
                | 'g'
                | 'r'
                | 'p'
                | 'P'
                | 'X'
                | 'D'
                | 'C'
                | 'S'
                | 'Y'
                | 'R'
        )
    )
}

fn is_change(action: Action, visual: bool) -> bool {
    match action {
        Action::Operate(Op::Yank, _)
        | Action::Move(_)
        | Action::Visual(_)
        | Action::Undo
        | Action::Redo
        | Action::Repeat
        | Action::Cancel
        | Action::SwapEnds => false,
        Action::Put(_) if visual => true,
        _ => true,
    }
}

// ── The command parser ───────────────────────────────────────────────────

/// A count at the front: digits, not starting with 0.
fn take_count(keys: &[Key]) -> (Option<usize>, &[Key]) {
    let mut n: Option<usize> = None;
    let mut i = 0;
    while let Some(Key::Char(c)) = keys.get(i) {
        match c.to_digit(10) {
            Some(d) if !(d == 0 && n.is_none()) => {
                n = Some(n.unwrap_or(0).saturating_mul(10).saturating_add(d as usize));
                i += 1;
            }
            _ => break,
        }
    }
    (n, &keys[i..])
}

fn multiply(a: Option<usize>, b: Option<usize>) -> Option<usize> {
    match (a, b) {
        (None, None) => None,
        _ => Some(a.unwrap_or(1).saturating_mul(b.unwrap_or(1))),
    }
}

/// A motion at the start of `keys`: the motion and how many keys it took, or
/// None if more keys are needed, or Err if they aren't a motion.
fn parse_motion(keys: &[Key]) -> Result<Option<(Motion, usize)>, ()> {
    let Some(&k) = keys.first() else {
        return Ok(None);
    };
    let m = match k {
        Key::Char('h') | Key::Left | Key::Backspace | Key::Ctrl('h') => Motion::Left,
        Key::Char('l') | Key::Right | Key::Char(' ') => Motion::Right,
        Key::Char('j') | Key::Down | Key::Ctrl('j') | Key::Ctrl('n') => Motion::Down,
        Key::Char('k') | Key::Up | Key::Ctrl('p') => Motion::Up,
        Key::Char('0') | Key::Home => Motion::LineStart,
        Key::Char('^') => Motion::FirstNonBlank,
        Key::Char('$') | Key::End => Motion::LineEnd,
        Key::Char('|') => Motion::Column,
        Key::Char('G') => Motion::GotoLast,
        Key::Char('+') | Key::Enter | Key::Ctrl('m') => Motion::NextLine,
        Key::Char('-') => Motion::PrevLine,
        Key::Char('_') => Motion::CurrentLine,
        Key::Char('w') => Motion::Word(false),
        Key::Char('W') => Motion::Word(true),
        Key::Char('b') => Motion::WordBack(false),
        Key::Char('B') => Motion::WordBack(true),
        Key::Char('e') => Motion::WordEnd(false),
        Key::Char('E') => Motion::WordEnd(true),
        Key::Char(';') => Motion::RepeatFind(false),
        Key::Char(',') => Motion::RepeatFind(true),
        Key::Char('}') => Motion::ParagraphForward,
        Key::Char('{') => Motion::ParagraphBack,
        Key::Char('%') => Motion::Match,
        Key::Char(c @ ('f' | 'F' | 't' | 'T')) => {
            let kind = match c {
                'f' => Find::Forward,
                'F' => Find::Backward,
                't' => Find::Till,
                _ => Find::TillBack,
            };
            return match keys.get(1) {
                None => Ok(None),
                Some(Key::Char(ch)) => Ok(Some((Motion::Find(kind, *ch), 2))),
                Some(Key::Tab) => Ok(Some((Motion::Find(kind, '\t'), 2))),
                Some(_) => Err(()),
            };
        }
        Key::Char('g') => {
            return match keys.get(1) {
                None => Ok(None),
                Some(Key::Char('g')) => Ok(Some((Motion::GotoFirst, 2))),
                Some(Key::Char('_')) => Ok(Some((Motion::LastNonBlank, 2))),
                Some(Key::Char('e')) => Ok(Some((Motion::WordEndBack(false), 2))),
                Some(Key::Char('E')) => Ok(Some((Motion::WordEndBack(true), 2))),
                Some(_) => Err(()),
            };
        }
        _ => return Err(()),
    };
    Ok(Some((m, 1)))
}

fn parse(keys: &[Key], visual: bool) -> Parse {
    let (count, rest) = take_count(keys);
    let Some(&first) = rest.first() else {
        return Parse::Incomplete;
    };
    let done = |action| Parse::Done(Command { count, action });
    let op = |k: Key, second: Option<&Key>| -> Option<Option<Op>> {
        // Some(Some(op)): an operator; Some(None): needs another key.
        match k {
            Key::Char('d') => Some(Some(Op::Delete)),
            Key::Char('c') => Some(Some(Op::Change)),
            Key::Char('y') => Some(Some(Op::Yank)),
            Key::Char('>') => Some(Some(Op::ShiftRight)),
            Key::Char('<') => Some(Some(Op::ShiftLeft)),
            Key::Char('g') => match second {
                None => Some(None),
                Some(Key::Char('u')) => Some(Some(Op::Lower)),
                Some(Key::Char('U')) => Some(Some(Op::Upper)),
                Some(Key::Char('~')) => Some(Some(Op::Toggle)),
                _ => None,
            },
            _ => None,
        }
    };

    if visual {
        let act = match first {
            Key::Char('d' | 'x') | Key::Delete => {
                Some(Action::Operate(Op::Delete, Some(Motion::Right)))
            }
            Key::Char('X' | 'D') => Some(Action::Operate(Op::Delete, None)),
            Key::Char('c' | 's') => Some(Action::Operate(Op::Change, Some(Motion::Right))),
            Key::Char('C' | 'S' | 'R') => Some(Action::Operate(Op::Change, None)),
            Key::Char('y') => Some(Action::Operate(Op::Yank, Some(Motion::Right))),
            Key::Char('Y') => Some(Action::Operate(Op::Yank, None)),
            Key::Char('>') => Some(Action::Operate(Op::ShiftRight, None)),
            Key::Char('<') => Some(Action::Operate(Op::ShiftLeft, None)),
            Key::Char('~') => Some(Action::ToggleCase),
            Key::Char('u') => Some(Action::Operate(Op::Lower, Some(Motion::Right))),
            Key::Char('U') => Some(Action::Operate(Op::Upper, Some(Motion::Right))),
            Key::Char('J') => Some(Action::Join(true)),
            Key::Char('p' | 'P') => Some(Action::Put(false)),
            Key::Char('o' | 'O') => Some(Action::SwapEnds),
            Key::Char('v') => Some(Action::Visual(false)),
            Key::Char('V') => Some(Action::Visual(true)),
            Key::Esc | Key::Ctrl('c') => Some(Action::Cancel),
            _ => None,
        };
        if let Some(a) = act {
            return done(a);
        }
        match (first, rest.get(1)) {
            (Key::Char('r'), None) => return Parse::Incomplete,
            (Key::Char('r'), Some(Key::Char(c))) => return done(Action::ReplaceChar(*c)),
            (Key::Char('g'), Some(Key::Char('J'))) => return done(Action::Join(false)),
            (Key::Char('g'), Some(Key::Char('u'))) => {
                return done(Action::Operate(Op::Lower, Some(Motion::Right)));
            }
            (Key::Char('g'), Some(Key::Char('U'))) => {
                return done(Action::Operate(Op::Upper, Some(Motion::Right)));
            }
            (Key::Char('g'), Some(Key::Char('~'))) => return done(Action::ToggleCase),
            _ => {}
        }
        return match parse_motion(rest) {
            Ok(None) => Parse::Incomplete,
            Ok(Some((m, _))) => done(Action::Move(m)),
            Err(()) => Parse::Invalid,
        };
    }

    // Normal mode: an operator, then a count and a motion, or doubled.
    match op(first, rest.get(1)) {
        Some(None) => {}
        Some(Some(o)) => {
            let skip = if first == Key::Char('g') { 2 } else { 1 };
            let after = &rest[skip..];
            let (count2, motion_keys) = take_count(after);
            let count = multiply(count, count2);
            let Some(&m0) = motion_keys.first() else {
                return Parse::Incomplete;
            };
            // Doubled: dd, cc, yy, >>, <<, guu/gugu, gUU/gUgU, g~~/g~g~.
            let doubled = match (first, rest.get(1)) {
                (Key::Char('g'), Some(&k2)) => {
                    m0 == k2 || (m0 == Key::Char('g') && motion_keys.get(1) == Some(&k2))
                }
                _ => m0 == first,
            };
            if doubled {
                if first == Key::Char('g') && m0 == Key::Char('g') && motion_keys.len() == 1 {
                    return Parse::Incomplete;
                }
                return Parse::Done(Command {
                    count,
                    action: Action::Operate(o, None),
                });
            }
            return match parse_motion(motion_keys) {
                Ok(None) => Parse::Incomplete,
                Ok(Some((m, _))) => Parse::Done(Command {
                    count,
                    action: Action::Operate(o, Some(m)),
                }),
                Err(()) => Parse::Invalid,
            };
        }
        None => {}
    }
    let simple = match first {
        Key::Char('x') | Key::Delete => Some(Action::Operate(Op::Delete, Some(Motion::Right))),
        Key::Char('X') => Some(Action::Operate(Op::Delete, Some(Motion::Left))),
        Key::Char('D') => Some(Action::Operate(Op::Delete, Some(Motion::LineEnd))),
        Key::Char('C') => Some(Action::Operate(Op::Change, Some(Motion::LineEnd))),
        Key::Char('Y') => Some(Action::Operate(Op::Yank, Some(Motion::LineEnd))),
        Key::Char('s') => Some(Action::Operate(Op::Change, Some(Motion::Right))),
        Key::Char('S') => Some(Action::Operate(Op::Change, None)),
        Key::Char('i') => Some(Action::Insert(Insert::Before)),
        Key::Char('a') => Some(Action::Insert(Insert::After)),
        Key::Char('I') => Some(Action::Insert(Insert::LineStart)),
        Key::Char('A') => Some(Action::Insert(Insert::LineEnd)),
        Key::Char('o') => Some(Action::Insert(Insert::Below)),
        Key::Char('O') => Some(Action::Insert(Insert::Above)),
        Key::Char('R') => Some(Action::Replace),
        Key::Char('J') => Some(Action::Join(true)),
        Key::Char('~') => Some(Action::ToggleCase),
        Key::Char('p') => Some(Action::Put(false)),
        Key::Char('P') => Some(Action::Put(true)),
        Key::Char('v') => Some(Action::Visual(false)),
        Key::Char('V') => Some(Action::Visual(true)),
        Key::Char('u') => Some(Action::Undo),
        Key::Ctrl('r') => Some(Action::Redo),
        Key::Char('.') => Some(Action::Repeat),
        Key::Esc => Some(Action::Cancel),
        _ => None,
    };
    if let Some(a) = simple {
        return done(a);
    }
    match (first, rest.get(1)) {
        (Key::Char('r'), None) => return Parse::Incomplete,
        (Key::Char('r'), Some(Key::Char(c))) => return done(Action::ReplaceChar(*c)),
        (Key::Char('r'), Some(Key::Tab)) => return done(Action::ReplaceChar('\t')),
        (Key::Char('r'), Some(_)) => return Parse::Invalid,
        (Key::Char('g'), Some(Key::Char('J'))) => return done(Action::Join(false)),
        _ => {}
    }
    match parse_motion(rest) {
        Ok(None) => Parse::Incomplete,
        Ok(Some((m, _))) => done(Action::Move(m)),
        Err(()) => Parse::Invalid,
    }
}
