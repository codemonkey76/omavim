//! Visual block mode (CTRL-V): the block a selection covers, in lines and
//! screen columns, and the operators on it, as Neovim's block_prep() and
//! the block parts of its operators and put.

use super::{Action, Command, Op, R, Vim};
use crate::TextModel;
use crate::text::{self, char_width, last_line, line_len, line_text};

/// The lines and screen columns (both ends in) a block covers, as Vim's
/// do_pending_operator works them out (with 'linebreak' off). `to_end`:
/// after `$`, to each line's end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Area {
    pub top: usize,
    pub bottom: usize,
    pub start_vcol: usize,
    pub end_vcol: usize,
    pub to_end: bool,
}

/// What a block covers on one line (Vim's struct block_def): the chars
/// `textcol..textcol + textlen`, and spaces for the parts of a tab or wide
/// char it cuts through.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct BlockDef {
    pub textcol: usize,
    pub textlen: usize,
    pub startspaces: usize,
    pub endspaces: usize,
    pub is_short: bool,
    pub is_one_char: bool,
    pub start_vcol: usize,
    pub end_vcol: usize,
    pub start_char_vcols: usize,
    pub end_char_vcols: usize,
    /// The blanks just before the block: their screen columns and chars.
    pub pre_whitesp: usize,
    pub pre_whitesp_c: usize,
}

/// Which operator block_prep() works for (it matters at the edges).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Other,
    Insert,
    Append,
    Replace,
    LShift,
}

impl Vim {
    /// A char's screen columns: where it starts and ends (at a line's end,
    /// both where the line ends). Vim's getvvcol, 'linebreak' off.
    pub(super) fn vcols(&self, t: &dyn TextModel, line: usize, col: usize) -> (usize, usize) {
        let ts = self.tabstop;
        let start = text::vcol(t, line, col, ts);
        match text::char_at(t, line, col) {
            Some(c) => (start, start + char_width(c, start, ts) - 1),
            None => (start, start),
        }
    }

    /// Where each char of a line starts on screen, as shown (and where the
    /// line ends).
    fn shown_starts(&self, t: &dyn TextModel, line: usize) -> Vec<usize> {
        if self.width == 0 || self.no_lbr {
            let len = line_len(t, line);
            (0..=len)
                .map(|c| text::vcol(t, line, c, self.tabstop))
                .collect()
        } else {
            self.layout(t, line).vcols
        }
    }

    /// A char's screen columns as shown, with 'linebreak' (what visual mode
    /// and put go by; only operators turn it off).
    pub(super) fn vcols_shown(&self, t: &dyn TextModel, line: usize, col: usize) -> (usize, usize) {
        if self.width == 0 || self.no_lbr {
            return self.vcols(t, line, col);
        }
        let l = self.layout(t, line);
        let n = l.vcols.len() - 1;
        if col >= n {
            (l.vcols[n], l.vcols[n])
        } else {
            (l.vcols[col], l.vcols[col + 1] - 1)
        }
    }

    /// The column of the char at screen column `vcol` (the last char if the
    /// line is shorter): Vim's coladvance in normal mode.
    pub(super) fn col_at_vcol(&self, t: &dyn TextModel, line: usize, vcol: usize) -> usize {
        text::col_at_vcol(t, line, vcol, self.tabstop).min(line_len(t, line).saturating_sub(1))
    }

    /// The block the selection (anchor to cursor) covers.
    pub(super) fn block_area(&self, t: &dyn TextModel) -> Area {
        let (al, ac) = text::line_col(t, self.anchor.min(t.len_chars()));
        let (cl, cc) = text::line_col(t, self.cursor.min(t.len_chars()));
        let (s1, e1) = self.vcols(t, al, ac);
        let (s2, e2) = self.vcols(t, cl, cc);
        let (top, bottom) = (al.min(cl), al.max(cl));
        let to_end = self.want == Some(usize::MAX);
        if let Some(width) = self.redo_block_width
            && !to_end
        {
            // Repeated: from where `.` was typed, as wide as before.
            return Area {
                top,
                bottom,
                start_vcol: s1,
                end_vcol: s1 + width - 1,
                to_end,
            };
        }
        let end_vcol = if to_end {
            // As far as the longest line goes.
            (top..=bottom)
                .map(|l| text::vcol(t, l, line_len(t, l), self.tabstop))
                .max()
                .unwrap_or(0)
        } else {
            e1.max(e2)
        };
        Area {
            top,
            bottom,
            start_vcol: s1.min(s2),
            end_vcol,
            to_end,
        }
    }

    /// Vim's block_prep(): what the block covers on `line`.
    pub(super) fn block_prep(
        &self,
        t: &dyn TextModel,
        a: &Area,
        line: usize,
        is_del: bool,
        kind: Kind,
    ) -> BlockDef {
        let ts = self.tabstop;
        let chars: Vec<char> = line_text(t, line).chars().collect();
        let len = chars.len();
        let mut bd = BlockDef::default();
        let mut vcol = 0;
        let mut incr = 0;
        let mut i = 0;
        let mut prev_pstart = 0;
        while vcol < a.start_vcol && i < len {
            incr = char_width(chars[i], vcol, ts);
            vcol += incr;
            if chars[i] == ' ' || chars[i] == '\t' {
                bd.pre_whitesp += incr;
                bd.pre_whitesp_c += 1;
            } else {
                bd.pre_whitesp = 0;
                bd.pre_whitesp_c = 0;
            }
            prev_pstart = i;
            i += 1;
        }
        bd.start_vcol = vcol;
        let mut pstart = i;
        bd.start_char_vcols = incr;
        if bd.start_vcol < a.start_vcol {
            // The line ends before the block.
            bd.end_vcol = bd.start_vcol;
            bd.is_short = true;
            if !is_del || kind == Kind::Append {
                bd.endspaces = a.end_vcol - a.start_vcol + 1;
            }
        } else {
            bd.startspaces = bd.start_vcol - a.start_vcol;
            if is_del && bd.startspaces > 0 {
                bd.startspaces = bd.start_char_vcols - bd.startspaces;
            }
            let mut pend = pstart;
            bd.end_vcol = bd.start_vcol;
            if bd.end_vcol > a.end_vcol {
                // All in one char.
                bd.is_one_char = true;
                match kind {
                    Kind::Insert => bd.endspaces = bd.start_char_vcols - bd.startspaces,
                    Kind::Append => {
                        bd.startspaces += a.end_vcol - a.start_vcol + 1;
                        bd.endspaces = bd.start_char_vcols - bd.startspaces;
                    }
                    _ => {
                        bd.startspaces = a.end_vcol - a.start_vcol + 1;
                        if is_del && kind != Kind::LShift {
                            // A tab split in two.
                            bd.startspaces = bd.start_char_vcols - (bd.start_vcol - a.start_vcol);
                            bd.endspaces = bd.end_vcol - a.end_vcol - 1;
                        }
                    }
                }
            } else {
                let mut v = bd.end_vcol;
                let mut j = pend;
                let mut prev_pend = pend;
                while v <= a.end_vcol && j < len {
                    prev_pend = j;
                    incr = char_width(chars[j], v, ts);
                    v += incr;
                    j += 1;
                }
                bd.end_vcol = v;
                pend = j;
                if bd.end_vcol <= a.end_vcol
                    && (!is_del || kind == Kind::Append || kind == Kind::Replace)
                {
                    // The line ends in the block.
                    bd.is_short = true;
                    if kind == Kind::Append {
                        bd.endspaces = a.end_vcol - bd.end_vcol + 1;
                    }
                } else if bd.end_vcol > a.end_vcol {
                    bd.endspaces = bd.end_vcol - a.end_vcol - 1;
                    if !is_del && bd.endspaces > 0 {
                        bd.endspaces = incr - bd.endspaces;
                        if pend != pstart {
                            pend = prev_pend;
                        }
                    }
                }
            }
            bd.end_char_vcols = incr;
            if is_del && bd.startspaces > 0 {
                pstart = prev_pstart;
            }
            bd.textlen = pend - pstart;
        }
        bd.textcol = pstart;
        bd
    }

    /// The corners Vim's operators work from: top left, bottom right.
    fn block_corners(&self, t: &dyn TextModel, a: &Area) -> ((usize, usize), (usize, usize)) {
        let start = (a.top, self.col_at_vcol(t, a.top, a.start_vcol));
        let end = (a.bottom, self.col_at_vcol(t, a.bottom, a.end_vcol));
        (start, end)
    }

    /// The block's text, a line each, and its width (Vim's y_width).
    fn block_text(&self, t: &dyn TextModel, a: &Area) -> (String, usize) {
        let mut lines = Vec::new();
        for l in a.top..=a.bottom {
            let bd = self.block_prep(t, a, l, false, Kind::Other);
            let chars: Vec<char> = line_text(t, l).chars().collect();
            let mut s = " ".repeat(bd.startspaces);
            s.extend(&chars[bd.textcol..(bd.textcol + bd.textlen).min(chars.len())]);
            s.push_str(&" ".repeat(bd.endspaces));
            lines.push(s);
        }
        let mut width = a.end_vcol - a.start_vcol;
        if a.to_end && width > 0 {
            width -= 1;
        }
        (lines.join("\n"), width)
    }

    /// `y` on a block: into the registers as a block. The cursor goes to
    /// its top left.
    pub(super) fn yank_block(&mut self, t: &dyn TextModel, a: &Area) -> R {
        let (text, width) = self.block_text(t, a);
        self.store_block(text, width, false);
        let (start, end) = self.block_corners(t, a);
        self.marks.op_start = Some(self.mk(t, start));
        self.marks.op_end = Some(self.mk(t, end));
        self.cursor = text::pos(t, start.0, start.1);
        self.want = None;
        Ok(())
    }

    /// `d` on a block: its text goes (into the registers), what's left of
    /// a tab or wide char it cut becoming spaces.
    pub(super) fn delete_block(&mut self, t: &mut dyn TextModel, a: &Area) -> R {
        let (text, width) = self.block_text(t, a);
        self.store_block(text, width, true);
        let (start, _) = self.block_corners(t, a);
        let mut cursor = text::pos(t, start.0, start.1);
        // (Undo comes back to the top left: Vim saves the block's lines.)
        self.cursor = cursor;
        self.begin_group();
        self.block_undo_top(a);
        for l in a.top..=a.bottom {
            let bd = self.block_prep(t, a, l, true, Kind::Other);
            if bd.textlen == 0 {
                continue;
            }
            let from = t.line_to_char(l) + bd.textcol;
            if l == a.top {
                cursor = from + bd.startspaces;
            }
            let spaces = " ".repeat(bd.startspaces + bd.endspaces);
            self.edit(t, from..from + bd.textlen, &spaces);
        }
        self.cursor = cursor;
        self.clamp(t);
        self.want = None;
        self.marks.op_start = Some(self.mk(t, start));
        self.marks.op_end = Some(self.mk(t, (a.bottom, start.1)));
        Ok(())
    }

    /// Put a block register at the cursor (after its char, unless
    /// `before`): each of its lines into a line from the cursor's down,
    /// at the same screen column, `count` times (Vim's do_put for a block).
    pub(super) fn put_block(
        &mut self,
        t: &mut dyn TextModel,
        text: &str,
        width: usize,
        before: bool,
        count: usize,
    ) -> R {
        let ts = self.tabstop;
        let (line, col) = text::line_col(t, self.cursor.min(t.len_chars()));
        let (start_v, end_v) = self.vcols_shown(t, line, col);
        let on_char = text::char_at(t, line, col).is_some();
        let (vcol, base) = if !before && on_char {
            (end_v + 1, col + 1)
        } else {
            (start_v, col)
        };
        let mut cursor_col = base;
        self.begin_group();
        let lines: Vec<&str> = text.split('\n').collect();
        let mut end = (line, 0);
        for (i, piece) in lines.iter().enumerate() {
            let l = line + i;
            if l > last_line(t) {
                let at = t.len_chars();
                self.edit(t, at..at, "\n");
            }
            let chars: Vec<char> = line_text(t, l).chars().collect();
            // Find where the block goes on this line (in the columns shown).
            let starts = self.shown_starts(t, l);
            let mut v = 0;
            let mut j = 0;
            let mut incr = 0;
            while v < vcol && j < chars.len() {
                incr = starts[j + 1] - starts[j];
                v = starts[j + 1];
                j += 1;
            }
            let mut textcol = j;
            let short = v < vcol || (v == vcol && j >= chars.len());
            let (mut startspaces, mut endspaces, mut delcount) = (0, 0, 0);
            if v < vcol {
                startspaces = vcol - v;
            } else if v > vcol {
                endspaces = v - vcol;
                startspaces = incr - endspaces;
                textcol -= 1;
                delcount = 1;
                if chars[textcol] != '\t' {
                    // Only a tab can be split into spaces.
                    delcount = 0;
                    endspaces = 0;
                }
            }
            let piece_width: usize = {
                let mut w = 0;
                for c in piece.chars() {
                    w += char_width(c, 0, ts);
                }
                w
            };
            let spaces = (width + 1).saturating_sub(piece_width);
            let mut new = " ".repeat(startspaces);
            for k in 0..count {
                new.push_str(piece);
                // Trailing spaces only if there's text after.
                if k < count - 1 || !short {
                    new.push_str(&" ".repeat(spaces));
                }
            }
            new.push_str(&" ".repeat(endspaces));
            let from = t.line_to_char(l) + textcol;
            self.edit(t, from..from + delcount, &new);
            if i == 0 {
                cursor_col = base + startspaces;
            }
            end = (l, (textcol + new.chars().count()).saturating_sub(1));
        }
        self.marks.op_start = Some(self.mk(t, (line, cursor_col.min(line_len(t, line)))));
        self.marks.op_end = Some(self.mk(t, end));
        self.cursor = text::pos(t, line, cursor_col.min(line_len(t, line)));
        self.clamp(t);
        self.want = None;
        Ok(())
    }
}

impl Vim {
    /// A command in visual block mode, when it's the block's own: None for
    /// the ones that work as in the other visual modes.
    pub(super) fn run_block(
        &mut self,
        t: &mut dyn TextModel,
        cmd: &Command,
        exit: fn(&mut Self),
    ) -> Option<R> {
        let mut action = cmd.action;
        if let Action::OperateToEnd(op) = action {
            // `D`, `C`: to the end of each line.
            self.want = Some(usize::MAX);
            action = Action::Operate(op, Some(super::Motion::Right));
        }
        Some(match action {
            Action::SwapEnds(true) => {
                self.swap_corners(t);
                Ok(())
            }
            Action::Operate(Op::Yank, _) => {
                let a = self.block_area(t);
                exit(self);
                self.yank_block(t, &a)
            }
            Action::Operate(Op::Delete, _) => {
                let a = self.block_area(t);
                exit(self);
                self.delete_block(t, &a)
            }
            // `c`, `s`, `C` (`S` and `R` are linewise).
            Action::Operate(Op::Change, Some(_)) => {
                let a = self.block_area(t);
                exit(self);
                self.block_change(t, &a)
            }
            // `S` and `R`: linewise, as in the other visual modes.
            Action::Operate(Op::Change, None) => return None,
            Action::Insert(super::Insert::LineStart) => self.block_insert_start(t, false),
            Action::Insert(super::Insert::LineEnd) => self.block_insert_start(t, true),
            Action::ReplaceChar(c) => {
                let a = self.block_area(t);
                exit(self);
                self.replace_block(t, &a, c)
            }
            Action::ToggleCase => {
                let a = self.block_area(t);
                exit(self);
                self.case_block(t, &a, Op::Toggle)
            }
            Action::Operate(op @ (Op::Lower | Op::Upper | Op::Toggle), _) => {
                let a = self.block_area(t);
                exit(self);
                self.case_block(t, &a, op)
            }
            Action::Operate(op @ (Op::ShiftRight | Op::ShiftLeft), _) => {
                let a = self.block_area(t);
                exit(self);
                self.shift_block(t, &a, op == Op::ShiftLeft, cmd.count.unwrap_or(1))
            }
            Action::Put(before) => {
                let a = self.block_area(t);
                let end_line = text::line_col(t, self.cursor.min(t.len_chars())).0;
                exit(self);
                self.put_over_block(t, &a, end_line, before, cmd.count.unwrap_or(1))
            }
            Action::AddSub { sub, progressive } => {
                let a = self.block_area(t);
                let start = self.block_corners(t, &a).0;
                exit(self);
                self.cursor = text::pos(t, start.0, start.1);
                let r = self.add_sub_visual(
                    t,
                    sub,
                    cmd.count.unwrap_or(1),
                    progressive,
                    (a.top, 0),
                    (a.bottom, 0),
                    false,
                    a.to_end,
                    Some(&a),
                );
                self.cursor = text::pos(t, start.0, start.1);
                self.clamp(t);
                self.want = None;
                r
            }
            // `gq` and the others work on the lines.
            Action::Operate(Op::Format | Op::FormatKeep, _) => return None,
            _ => return None,
        })
    }

    /// `O` in block mode: to the other corner on the cursor's line (Vim's
    /// v_swap_corners).
    fn swap_corners(&mut self, t: &dyn TextModel) {
        let (cl, cc) = text::line_col(t, self.cursor.min(t.len_chars()));
        let (al, ac) = text::line_col(t, self.anchor.min(t.len_chars()));
        let (s1, e1) = self.vcols_shown(t, cl, cc);
        let (s2, e2) = self.vcols_shown(t, al, ac);
        let (left, right) = (s1.min(s2), e1.max(e2));
        let visual_col = |v: &Self, l: usize, vcol: usize| v.col_on(t, l, vcol);
        self.anchor = text::pos(t, al, visual_col(self, al, left));
        let col = visual_col(self, cl, right);
        if col == cc {
            // Already on the right: the other way round.
            self.anchor = text::pos(t, al, visual_col(self, al, right));
            self.cursor = text::pos(t, cl, visual_col(self, cl, left));
            self.want = Some(left);
        } else {
            self.cursor = text::pos(t, cl, col);
            self.want = Some(right);
        }
    }
}

impl Vim {
    /// In visual block mode, the block as it's shown: its first and last
    /// lines, and its screen columns (both in, with 'linebreak'; the end
    /// `usize::MAX` after `$`), for the app to highlight.
    pub fn visual_block(&self, t: &dyn TextModel) -> Option<(usize, usize, usize, usize)> {
        if self.mode != crate::Mode::VisualBlock {
            return None;
        }
        let (al, ac) = text::line_col(t, self.anchor.min(t.len_chars()));
        let (cl, cc) = text::line_col(t, self.cursor.min(t.len_chars()));
        let (s1, e1) = self.vcols_shown(t, al, ac);
        let (s2, e2) = self.vcols_shown(t, cl, cc);
        let end = if self.want == Some(usize::MAX) {
            usize::MAX
        } else {
            e1.max(e2)
        };
        Some((al.min(cl), al.max(cl), s1.min(s2), end))
    }
}

/// A block insert (`I`, `A`) or change (`c`) under way: the text typed on
/// the first line goes into the others when insert mode ends.
#[derive(Debug, Clone)]
pub(super) struct BlockInsert {
    area: Area,
    kind: Kind,
    change: bool,
    /// Where the block starts on the first line (Vim's oap->start.col).
    start_col: usize,
    /// The first line's block, and its length, before the insert.
    textcol: usize,
    textlen: usize,
    pre_textlen: isize,
    pre_indent: usize,
}

impl Vim {
    /// `I` and `A` on a block: insert before it (after it) on its first
    /// line, and on the others when insert mode ends (Vim's op_insert).
    pub(super) fn block_insert_start(&mut self, t: &mut dyn TextModel, append: bool) -> R {
        let a = self.block_area(t);
        let kind = if append { Kind::Append } else { Kind::Insert };
        let start_col = self.col_at_vcol(t, a.top, a.start_vcol);
        let mut bd = self.block_prep(t, &a, a.top, true, kind);
        // The length of the text after where the insert goes.
        let mut pre_textlen = line_len(t, a.top) as isize - bd.textcol as isize;
        if append {
            pre_textlen -= bd.textlen as isize;
        }
        self.mode = crate::Mode::Normal;
        self.cursor = text::pos(t, a.top, start_col);
        self.begin_group();
        if append {
            // To the char after the block (the first line made long enough
            // if it's short).
            let len = line_len(t, a.top);
            let mut col = start_col;
            while col < len && col < bd.textcol + bd.textlen {
                col += 1;
            }
            self.cursor = text::pos(t, a.top, col);
            if bd.is_short && !a.to_end {
                let at = self.cursor;
                self.edit(t, at..at, &" ".repeat(bd.endspaces));
                self.cursor = at + bd.endspaces;
                bd.textlen += bd.endspaces;
            }
        }
        self.want = None;
        let mut s = super::Session::new(None, 1, self.cursor);
        s.block = Some(BlockInsert {
            area: a,
            kind,
            change: false,
            start_col,
            textcol: bd.textcol,
            textlen: bd.textlen,
            pre_textlen,
            pre_indent: 0,
        });
        self.session = Some(s);
        self.mode = crate::Mode::Insert;
        Ok(())
    }

    /// `c` on a block: its text goes, then what's typed on the first line
    /// goes into the others too (Vim's op_change).
    pub(super) fn block_change(&mut self, t: &mut dyn TextModel, a: &Area) -> R {
        let start_col = self.col_at_vcol(t, a.top, a.start_vcol);
        self.delete_block(t, a)?;
        let (l, col) = text::line_col(t, self.cursor);
        let mut col = col;
        if start_col > col && line_len(t, l) > 0 {
            col += 1;
        }
        self.cursor = text::pos(t, l, col.min(line_len(t, l)));
        let line: Vec<char> = line_text(t, a.top).chars().collect();
        let pre_indent = line
            .iter()
            .take_while(|c| **c == ' ' || **c == '\t')
            .count();
        let mut s = super::Session::new(Some(super::Insert::Before), 1, self.cursor);
        s.block = Some(BlockInsert {
            area: *a,
            kind: Kind::Other,
            change: true,
            start_col,
            textcol: col,
            textlen: 0,
            pre_textlen: line.len() as isize,
            pre_indent,
        });
        self.session = Some(s);
        self.mode = crate::Mode::Insert;
        Ok(())
    }

    /// Insert mode ended: the text typed on the block's first line into
    /// the others.
    pub(super) fn block_insert_finish(&mut self, t: &mut dyn TextModel, bi: BlockInsert) {
        let a = bi.area;
        let first: Vec<char> = line_text(t, a.top).chars().collect();
        if bi.change {
            if a.top == a.bottom {
                return;
            }
            let mut textcol = bi.textcol;
            let mut pre_textlen = bi.pre_textlen;
            if textcol > bi.pre_indent {
                // Autoindent may have changed the indent.
                let new_indent = first
                    .iter()
                    .take_while(|c| **c == ' ' || **c == '\t')
                    .count();
                pre_textlen += new_indent as isize - bi.pre_indent as isize;
                textcol =
                    (textcol as isize + new_indent as isize - bi.pre_indent as isize) as usize;
            }
            let ins_len = first.len() as isize - pre_textlen;
            if ins_len <= 0 || textcol + ins_len as usize > first.len() {
                return;
            }
            let ins: String = first[textcol..textcol + ins_len as usize].iter().collect();
            for l in a.top + 1..=a.bottom {
                let bd = self.block_prep(t, &a, l, true, Kind::Other);
                if !bd.is_short {
                    let at = t.line_to_char(l) + bd.textcol;
                    self.edit(t, at..at, &ins);
                }
            }
            self.clamp(t);
            return;
        }
        // Insert or append: only if the cursor stayed on the first line.
        if text::line_col(t, self.cursor.min(t.len_chars())).0 != a.top {
            return;
        }
        let mut a = a;
        let mut append = bi.kind == Kind::Append;
        let mut pre_textlen = bi.pre_textlen;
        let mut start_col = bi.start_col;
        // The cursor moved before anything was typed: the block starts where
        // the insert did.
        if let Some((l, c)) = self.marks.op_start.map(|m| self.unmk(t, m))
            && l == a.top
            && !a.to_end
        {
            let v = self.vcols(t, l, c).0;
            if !append && start_col != c {
                start_col = c;
                pre_textlen -= v as isize - a.start_vcol as isize;
                a.start_vcol = v;
            } else if append && start_col >= c {
                start_col = c;
                pre_textlen += bi.textlen as isize;
                pre_textlen -= v as isize - a.start_vcol as isize;
                a.start_vcol = v;
                append = false;
            }
        }
        let kind = if append { Kind::Append } else { Kind::Insert };
        let mut textcol = bi.textcol;
        let mut textlen = bi.textlen;
        let bd2 = self.block_prep(t, &a, a.top, true, kind);
        if !a.to_end || bd2.textlen < textlen {
            let mut len2 = bd2.textlen;
            if append {
                pre_textlen += bd2.textlen as isize - textlen as isize;
                if bd2.endspaces > 0 {
                    len2 = len2.saturating_sub(1);
                }
            }
            textcol = bd2.textcol;
            textlen = len2;
        }
        let mut add = textcol;
        if append {
            add += textlen;
        }
        let add = add.min(first.len());
        let ins_len = (first.len() - add) as isize - pre_textlen;
        if pre_textlen < 0 || ins_len <= 0 {
            return;
        }
        let ins: String = first[add..add + ins_len as usize].iter().collect();
        self.block_insert_lines(t, &a, &ins, !append);
        self.cursor = text::pos(t, a.top, start_col.min(line_len(t, a.top)));
        self.clamp(t);
    }

    /// Vim's block_insert(): `s` into lines after the first, before the
    /// block (`before`) or after it.
    fn block_insert_lines(&mut self, t: &mut dyn TextModel, a: &Area, s: &str, before: bool) {
        let kind = if before { Kind::Insert } else { Kind::Append };
        for l in a.top + 1..=a.bottom {
            let bd = self.block_prep(t, a, l, true, kind);
            if bd.is_short && before {
                continue;
            }
            let old: Vec<char> = line_text(t, l).chars().collect();
            // Spaces before the text (for a tab or wide char it cuts, or to
            // reach the block on a short line), and where it goes.
            let (ts_val, spaces, offset);
            if before {
                ts_val = bd.start_char_vcols;
                spaces = bd.startspaces;
                offset = bd.textcol;
            } else {
                ts_val = bd.end_char_vcols;
                if !bd.is_short {
                    spaces = if bd.endspaces > 0 {
                        ts_val - bd.endspaces
                    } else {
                        0
                    };
                    offset = bd.textcol + bd.textlen - usize::from(spaces != 0);
                } else {
                    spaces = if a.to_end {
                        0
                    } else {
                        a.end_vcol + 1 - bd.end_vcol
                    };
                    offset = bd.textcol + bd.textlen;
                }
            }
            let mut new: String = " ".repeat(spaces);
            new.push_str(s);
            let mut skip = 0;
            if spaces > 0 && !bd.is_short && old.get(offset) == Some(&'\t') {
                // A tab cut in two: spaces for the rest of it.
                new.push_str(&" ".repeat(ts_val - spaces));
                skip = 1;
            }
            let at = t.line_to_char(l) + offset;
            self.edit(t, at..at + skip, &new);
            if l == a.bottom {
                let end = offset + new.chars().count();
                self.marks.op_end = Some(self.mk(t, (l, end)));
            }
        }
    }
}

/// Tabs and spaces from screen column `from` to `to` (Vim's
/// tabstop_fromto, without 'vartabstop').
fn tabs_spaces(from: usize, to: usize, ts: usize) -> (usize, usize) {
    let mut spaces = to - from;
    let mut tabs = 0;
    let first = ts - from % ts;
    if spaces >= first {
        spaces -= first;
        tabs += 1;
    }
    tabs += spaces / ts;
    spaces %= ts;
    (tabs, spaces)
}

impl Vim {
    /// `r{c}` on a block: every char in it `c` (a tab it cuts becomes
    /// spaces), as Vim's op_replace.
    pub(super) fn replace_block(&mut self, t: &mut dyn TextModel, a: &Area, c: char) -> R {
        let start = self.block_corners(t, a).0;
        self.cursor = text::pos(t, start.0, start.1);
        self.begin_group();
        self.block_undo_top(a);
        let wide = char_width(c, 0, self.tabstop) > 1;
        for l in a.top..=a.bottom {
            let mut bd = self.block_prep(t, a, l, true, Kind::Replace);
            if bd.textlen == 0 {
                continue;
            }
            let mut numc = a.end_vcol + 1 - a.start_vcol;
            if bd.is_short {
                numc -= a.end_vcol + 1 - bd.end_vcol;
            }
            if wide {
                if numc % 2 == 1 && !bd.is_short {
                    bd.endspaces += 1;
                }
                numc /= 2;
            }
            let mut new = " ".repeat(bd.startspaces);
            new.extend(std::iter::repeat_n(c, numc));
            if !bd.is_short {
                new.push_str(&" ".repeat(bd.endspaces));
            }
            let from = t.line_to_char(l) + bd.textcol;
            let to = if bd.is_short {
                t.line_to_char(l) + line_len(t, l)
            } else {
                from + bd.textlen
            };
            self.edit(t, from..to, &new);
        }
        self.cursor = text::pos(t, start.0, start.1);
        self.clamp(t);
        self.want = None;
        Ok(())
    }

    /// `~`, `u`, `U` on a block (Vim's op_tilde).
    pub(super) fn case_block(&mut self, t: &mut dyn TextModel, a: &Area, op: Op) -> R {
        let (start, end) = self.block_corners(t, a);
        self.cursor = text::pos(t, start.0, start.1);
        self.begin_group();
        self.block_undo_top(a);
        for l in a.top..=a.bottom {
            let bd = self.block_prep(t, a, l, false, Kind::Other);
            let from = t.line_to_char(l) + bd.textcol;
            self.map_case(t, from..from + bd.textlen, op);
        }
        self.marks.op_start = Some(self.mk(t, start));
        self.marks.op_end = Some(self.mk(t, end));
        self.cursor = text::pos(t, start.0, start.1);
        self.clamp(t);
        self.want = None;
        Ok(())
    }

    /// `>` and `<` on a block: blanks put in (taken out) where it starts,
    /// `count` shiftwidths, as Vim's op_shift and shift_block.
    pub(super) fn shift_block(
        &mut self,
        t: &mut dyn TextModel,
        a: &Area,
        left: bool,
        count: usize,
    ) -> R {
        let start = self.block_corners(t, a).0;
        self.cursor = text::pos(t, start.0, start.1);
        self.begin_group();
        self.block_undo_top(a);
        let ts = self.tabstop;
        let sw = if self.shiftwidth == 0 {
            ts
        } else {
            self.shiftwidth
        };
        for l in a.top..=a.bottom {
            if line_len(t, l) == 0 {
                continue;
            }
            let mut bd =
                self.block_prep(t, a, l, true, if left { Kind::LShift } else { Kind::Other });
            if bd.is_short {
                continue;
            }
            let old: Vec<char> = line_text(t, l).chars().collect();
            let white = |i: usize| matches!(old.get(i), Some(' ' | '\t'));
            let mut total = count * sw;
            let new_line: String = if !left {
                total += bd.pre_whitesp;
                let mut ws_vcol = bd.start_vcol - bd.pre_whitesp;
                let mut textstart = bd.textcol;
                if bd.startspaces > 0 {
                    if old.get(textstart).is_some_and(|c| c.len_utf8() == 1) {
                        textstart += 1;
                    } else {
                        ws_vcol = 0;
                        bd.startspaces = 0;
                    }
                }
                let mut vcol = bd.start_vcol;
                while white(textstart) {
                    let incr = char_width(old[textstart], vcol, ts);
                    textstart += 1;
                    total += incr;
                    vcol += incr;
                }
                let (tabs, spaces) = tabs_spaces(ws_vcol, ws_vcol + total, ts);
                let col_pre = bd.pre_whitesp_c - usize::from(bd.startspaces != 0);
                let textcol = bd.textcol - col_pre;
                let mut s: String = old[..textcol].iter().collect();
                s.push_str(&"\t".repeat(tabs));
                s.push_str(&" ".repeat(spaces));
                s.extend(&old[textstart..]);
                s
            } else {
                let mut non_white = bd.textcol + usize::from(bd.startspaces > 0);
                let mut non_white_col = bd.start_vcol;
                while white(non_white) {
                    non_white_col += char_width(old[non_white], non_white_col, ts);
                    non_white += 1;
                }
                let block_space = non_white_col - a.start_vcol;
                let destination = non_white_col - block_space.min(total);
                let mut end = bd.textcol;
                let mut width = bd.start_vcol;
                if bd.startspaces > 0 {
                    width -= bd.start_char_vcols;
                }
                while width < destination && end < old.len() {
                    let incr = char_width(old[end], width, ts);
                    if width + incr > destination {
                        break;
                    }
                    width += incr;
                    end += 1;
                }
                let fill = destination - width;
                let mut s: String = old[..end].iter().collect();
                s.push_str(&" ".repeat(fill));
                s.extend(&old[non_white.min(old.len())..]);
                s
            };
            if new_line != old.iter().collect::<String>() {
                let from = t.line_to_char(l);
                self.edit(t, from..from + old.len(), &new_line);
            }
        }
        let last = a.bottom;
        self.marks.op_start = Some(self.mk(t, start));
        self.marks.op_end = Some(self.mk(t, (last, line_len(t, last).saturating_sub(1))));
        // (Not moved back onto the line if it got shorter, as Vim.)
        self.cursor = text::pos(t, start.0, start.1.min(line_len(t, start.0)));
        self.keep_cursor = true;
        self.want = None;
        Ok(())
    }

    /// `p` (`P`: keeping the registers) over a block: the block goes, and
    /// the register goes in its place (Vim's nv_put in visual mode).
    pub(super) fn put_over_block(
        &mut self,
        t: &mut dyn TextModel,
        a: &Area,
        end_line: usize,
        keep: bool,
        count: usize,
    ) -> R {
        let reg = self.read_register().unwrap_or_default();
        let name = self.reg_name;
        // (Where the block starts, before it goes.)
        let start_col = self.col_at_vcol(t, a.top, a.start_vcol);
        self.reg_name = if keep { Some('_') } else { None };
        self.delete_block(t, a)?;
        self.reg_name = name;
        let (line, col) = text::line_col(t, self.cursor);
        // At a short line's end: after it.
        let forward = col < start_col;
        if let Some(width) = reg.block {
            return self.put_block(t, &reg.text, width, !forward, count);
        }
        if reg.linewise {
            // Lines: `p` after the line the selection ended on; `P` before
            // the block's first (after it, if the cursor had to move back).
            if keep {
                return self.put_register(t, &reg, !forward, count);
            }
            self.cursor = text::pos(t, end_line.min(last_line(t)), 0);
            return self.put_register(t, &reg, false, count);
        }
        if reg.text.contains('\n') {
            return self.put_register(t, &reg, !forward, count);
        }
        // One line of text: into each line of the block.
        let piece = reg.text.repeat(count);
        let n = piece.chars().count();
        if n == 0 {
            return Ok(());
        }
        let col = col + usize::from(forward && line_len(t, line) > 0);
        let vcol = self.vcols_shown(t, line, col).0;
        let mut first_col = col;
        for l in a.top..=a.bottom {
            let c = if l == line {
                col
            } else {
                let starts = self.shown_starts(t, l);
                match starts.iter().position(|&v| v >= vcol) {
                    Some(c) => c,
                    None => continue,
                }
            };
            if c > line_len(t, l) {
                continue;
            }
            if l == line {
                first_col = c;
            }
            let at = t.line_to_char(l) + c;
            self.edit(t, at..at, &piece);
        }
        self.marks.op_start = Some(self.mk(t, (line, first_col)));
        self.cursor = text::pos(t, line, first_col + n - 1);
        self.marks.op_end = Some(self.mk(t, (line, first_col + n - 1)));
        self.want = None;
        Ok(())
    }
}

impl Vim {
    /// Vim saves a block's lines for undo: undo comes back to its top.
    fn block_undo_top(&mut self, a: &Area) {
        if let Some(g) = self.group.as_mut() {
            g.top = Some(g.top.map_or(a.top, |l| l.min(a.top)));
        }
    }
}
