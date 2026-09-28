//! The view: which screen rows are shown, kept around the cursor as Neovim
//! keeps them (its move.c and normal.c, with 'wrap', 'linebreak' and
//! 'smoothscroll' on and 'scrolloff' 0), and the commands that move by
//! screen line or scroll: gj gk g0 g^ gm g$ H M L, CTRL-E CTRL-Y CTRL-D
//! CTRL-U CTRL-F CTRL-B, and the z commands.
//!
//! Neovim's names are kept (topline, skipcol, botline, plines...) so this
//! can be read against its source. Lines are 0-based here, and "skipcol" is
//! counted in rows: `self.top` is (topline, rows of it scrolled off).

use super::{Beep, Mode, R, Vim};
use crate::text::{self, is_blank, last_line, line_len};
use crate::{Pos, TextModel};

/// Columns the 'smoothscroll' marker ("<<<") covers at the top left when
/// the top line is partly scrolled off: the cursor can't be under it.
const MARKER: usize = 3;

/// A direction to scroll or move in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Dir {
    Forward,
    Backward,
}

/// The scrolling commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Scroll {
    /// CTRL-E (true) and CTRL-Y.
    Line(bool),
    /// CTRL-D (true) and CTRL-U.
    Half(bool),
    /// CTRL-F (true) and CTRL-B.
    Page(bool),
}

impl Vim {
    fn scrolling(&self) -> bool {
        self.width > 0 && self.height > 0
    }

    /// A line's width on screen, wrap padding included (Neovim's
    /// linetabsize).
    fn line_size(&self, t: &dyn TextModel, line: usize) -> usize {
        *self.layout(t, line).vcols.last().unwrap()
    }

    /// Screen rows a line takes (plines_win_nofill); `limit`: no more than
    /// the window's height.
    fn plines(&self, t: &dyn TextModel, line: usize, limit: bool) -> usize {
        let size = self.line_size(t, line);
        let w = self.width;
        let n = if size <= w {
            1
        } else {
            (size - w).div_ceil(w) + 1
        };
        if limit { n.min(self.height) } else { n }
    }

    /// Rows a line shows: less its scrolled-off rows if it's the top line.
    fn plines_correct_topline(&self, t: &dyn TextModel, line: usize, limit: bool) -> usize {
        let mut n = self.plines(t, line, false);
        if line == self.top.0 {
            n -= self.top.1.min(n);
        }
        if limit { n.min(self.height) } else { n }
    }

    /// The first line not wholly on screen (may be one past the last), and
    /// the rows left empty below the text (comp_botline).
    fn botline(&self, t: &dyn TextModel) -> (usize, usize) {
        let mut done = 0;
        let mut line = self.top.0;
        while line <= last_line(t) {
            let n = self.plines_correct_topline(t, line, true);
            if done + n > self.height {
                break;
            }
            done += n;
            line += 1;
        }
        (line, if done == 0 { 0 } else { self.height - done })
    }

    fn cline(&self, t: &dyn TextModel) -> usize {
        text::line_col(t, self.cursor).0
    }

    /// The cursor's screen column, wrap padding included (w_virtcol): on a
    /// tab in normal mode that's the tab's last cell, where the cursor shows.
    pub(super) fn virtcol(&self, t: &dyn TextModel) -> usize {
        let (l, c) = text::line_col(t, self.cursor);
        let v = self.vcol(t, l, c);
        let on_tab_end = text::char_at(t, l, c) == Some('\t')
            && match self.mode {
                Mode::Normal | Mode::OperatorPending => true,
                Mode::Visual | Mode::VisualLine => self.cursor > self.anchor,
                _ => false,
            };
        if on_tab_end {
            self.vcol(t, l, c + 1).saturating_sub(1).max(v)
        } else {
            v
        }
    }

    /// Where the char under the cursor starts on screen.
    fn virtcol_start(&self, t: &dyn TextModel) -> usize {
        let (l, c) = text::line_col(t, self.cursor);
        self.vcol(t, l, c)
    }

    /// The column j and k aim for (w_curswant), worked out if it's unset.
    fn curswant(&self, t: &dyn TextModel) -> usize {
        self.want.unwrap_or_else(|| self.virtcol(t))
    }

    /// Put the cursor on `line` at screen column `wcol`, as Vim's
    /// coladvance(): false if the column isn't reached (the line is
    /// shorter, or the column is inside a wide char or tab).
    fn coladvance(&mut self, t: &dyn TextModel, line: usize, wcol: usize) -> bool {
        let visual = matches!(self.mode, Mode::Visual | Mode::VisualLine);
        let len = line_len(t, line);
        let mut col = if wcol == usize::MAX {
            len
        } else {
            self.col_at(t, line, wcol)
        };
        if col >= len && len > 0 && !visual {
            col = len - 1;
        }
        self.cursor = text::pos(t, line, col);
        wcol == usize::MAX || self.vcol(t, line, col) >= wcol
    }

    /// After an edit to lines `first..=last` (as they were), which left
    /// `breaks` line breaks in their place (Neovim's changed_common).
    pub(super) fn changed_lines(
        &mut self,
        t: &dyn TextModel,
        first: usize,
        end: usize,
        last_after: isize,
    ) {
        if !self.scrolling() {
            return;
        }
        // The top line stays put, even if the lines it was showing went
        // (Neovim moves only other windows' top lines): the next
        // scroll_to_cursor brings the view back to the cursor. What it
        // skipped goes (Neovim's changed_common) when the change ends above
        // it, or it's changed and too short to show anything past that.
        let (top, skip) = self.top;
        let top_line = top.min(last_line(t));
        if skip > 0
            && (last_after < top as isize
                || (top >= first
                    && top < end
                    && self.line_size(t, top_line) <= skip * self.width + MARKER))
        {
            self.top.1 = 0;
        }
    }

    // ── Keeping the cursor in view ───────────────────────────────────────

    /// After a command: scroll so the cursor is on screen (update_topline).
    pub(super) fn scroll_to_cursor(&mut self, t: &dyn TextModel) {
        if !self.scrolling() {
            return;
        }
        let last = last_line(t);
        if self.top.0 <= last {
            self.top.1 = self.top.1.min(self.plines(t, self.top.0, false) - 1);
        }
        if last == 0 && line_len(t, 0) == 0 {
            self.top = (0, 0);
            return;
        }
        let cl = self.cline(t);
        let (top, skip) = self.top;
        let mut check_botline = false;
        let mut check_topline = false;
        if top > 0 || skip > 0 {
            if cl < top {
                check_topline = true;
            } else if skip > 0 && cl == top {
                // Hidden in the rows scrolled off, or under the marker.
                if skip * self.width + MARKER > self.virtcol_start(t) {
                    check_topline = true;
                }
            }
        }
        if check_topline {
            let halfheight = (self.height / 2).saturating_sub(1).max(2);
            let n = top as isize - cl as isize;
            if n >= halfheight as isize {
                self.scroll_cursor_halfway(t, false, false);
            } else {
                self.scroll_cursor_top(t, 1, false);
                check_botline = true;
            }
        } else {
            check_botline = true;
        }
        if check_botline {
            let (botline, _) = self.botline(t);
            if botline <= last && cl >= botline {
                let n = cl - botline + 1;
                if n <= self.height + 1 {
                    self.scroll_cursor_bot(t, 1, false);
                } else {
                    self.scroll_cursor_halfway(t, false, false);
                }
            }
        }
        // Neovim does this whenever it works out where the cursor shows.
        self.curs_columns(t);
    }

    /// A cursor in a top line taller than the window: scroll within it so
    /// the cursor shows (the skipcol part of curs_columns, 'scrolloff' 0).
    fn curs_columns(&mut self, t: &dyn TextModel) {
        let cl = self.cline(t);
        if cl != self.top.0 {
            return;
        }
        let (w, h) = (self.width as isize, self.height as isize);
        let v = self.virtcol(t) as isize;
        let prev = self.top.1 as isize;
        let mut skip = prev;
        let did_sub = skip > 0 && v >= skip * w;
        let mut wrow = v / w - if did_sub { skip } else { 0 };
        let plines = self.plines(t, cl, false) as isize;
        if !(wrow >= h || (skip > 0 && plines > h)) {
            return;
        }
        let mut extra = 0;
        if skip * w > v {
            extra = 1;
        }
        let last = plines - 1;
        let n = if last > wrow { wrow } else { last };
        if n >= h + skip {
            extra += 2;
        }
        if extra == 3 {
            let mut n = v / w;
            n = if n > h / 2 { n - h / 2 } else { 0 };
            n = n.min(last - h + 1);
            skip = n.max(0);
        } else if extra == 1 {
            let back = ((skip * w - v + w - 1) / w).min(skip);
            if back > 0 {
                skip -= back;
            }
        } else if extra == 2 {
            let mut end = n - h + 1;
            while end * w > v {
                end -= 1;
            }
            skip = skip.max(end);
        }
        wrow -= if did_sub { skip - prev } else { skip };
        if wrow >= h {
            skip += wrow - h + 1;
        }
        self.top.1 = skip.max(0) as usize;
    }

    /// After the cursor moved in the top line: make sure it isn't in the
    /// rows scrolled off, or under the marker (adjust_skipcol).
    pub(super) fn adjust_skipcol(&mut self, t: &dyn TextModel) {
        if !self.scrolling() || self.cline(t) != self.top.0 {
            return;
        }
        let cl = self.cline(t);
        if self.plines(t, cl, true) == self.height && self.plines(t, cl, false) <= self.height {
            self.top.1 = 0;
            return;
        }
        let v = self.virtcol(t);
        let mut scrolled = false;
        while self.top.1 > 0 && v < self.top.1 * self.width + MARKER {
            self.top.1 -= 1;
            scrolled = true;
        }
        if scrolled {
            return;
        }
        let row = (v - self.top.1 * self.width) / self.width;
        if row >= self.height {
            self.top.1 += row - self.height + 1;
        }
    }

    /// Put the cursor line at the top, or near it (scroll_cursor_top).
    fn scroll_cursor_top(&mut self, t: &dyn TextModel, min_scroll: usize, always: bool) {
        let (old_top, _) = self.top;
        let cl = self.cline(t);
        let mut used = self.plines(t, cl, true);
        let mut scrolled = if cl < old_top { used } else { 0 };
        let mut top = cl as isize - 1;
        let mut new_topline = cl;
        let mut extra = 0;
        while top >= 0 {
            let i = self.plines(t, top as usize, true);
            if (top as usize) < old_top {
                scrolled += i;
            }
            if new_topline >= old_top || scrolled > min_scroll {
                break;
            }
            used += i;
            if used > self.height {
                break;
            }
            extra += i;
            new_topline = top as usize;
            top -= 1;
        }
        let _ = extra;
        if used > self.height {
            self.scroll_cursor_halfway(t, false, false);
            return;
        }
        if new_topline < self.top.0 || always {
            self.top.0 = new_topline;
        }
        self.top.0 = self.top.0.min(cl);
        // A new top line starts unscrolled; so does the cursor's line if the
        // cursor is in the rows scrolled off.
        if self.top.0 != old_top || (self.top.0 == cl && self.top.1 * self.width >= self.virtcol(t))
        {
            self.top.1 = 0;
        }
    }

    /// Put the cursor line at the bottom, or near it (scroll_cursor_bot).
    /// `set_topbot`: first set the top from the cursor line up (for `zb`).
    fn scroll_cursor_bot(&mut self, t: &dyn TextModel, min_scroll: usize, set_topbot: bool) {
        let old = self.top;
        let cln = self.cline(t);
        let last = last_line(t);
        let h = self.height;
        let (botline, empty_rows) = if set_topbot {
            let mut used = 0;
            let mut skip = old.1;
            let mut loff = cln as isize + 1;
            loop {
                loff -= 1;
                if loff < 0 {
                    break;
                }
                let height = self.plines(t, loff as usize, false);
                if used + height > h {
                    // Too long to show whole: show its last rows.
                    if used < h {
                        self.top.0 = loff as usize;
                        skip = used + height - h;
                        used = h;
                    }
                    break;
                }
                self.top.0 = loff as usize;
                used += height;
            }
            // A skipcol left from before is dropped.
            self.top.1 = if skip != old.1 { skip } else { 0 };
            (cln + 1, if used == 0 { 0 } else { h - used })
        } else {
            self.botline(t)
        };

        let mut used = self.plines(t, cln, true);
        let mut scrolled: isize = 0;
        if cln >= botline {
            scrolled = used as isize;
            if cln == botline {
                scrolled -= empty_rows as isize;
            }
            // A top line taller than the window: its hidden rows count too.
            let top_plines = self.plines(t, self.top.0, false) as isize - self.top.1 as isize;
            if top_plines > h as isize {
                scrolled += top_plines - h as isize;
            }
        }
        let min = min_scroll as isize;
        let (mut loff, mut boff) = (cln, cln);
        while loff > 0 {
            if ((scrolled <= 0 || scrolled >= min) || boff >= last) && loff <= botline {
                break;
            }
            let before = loff;
            loff -= 1;
            let height = self.plines(t, loff, true);
            used += height;
            if used > h {
                break;
            }
            if loff >= botline {
                scrolled += height as isize;
                if loff == botline && before > botline {
                    scrolled -= empty_rows as isize;
                }
            }
            if boff < last {
                let before = boff;
                boff += 1;
                let height = self.plines(t, boff, true);
                used += height;
                if used > h {
                    break;
                }
                if scrolled < min && boff >= botline {
                    scrolled += height as isize;
                    if before < botline {
                        scrolled -= empty_rows as isize;
                    }
                }
            }
        }
        let line_count = if scrolled <= 0 {
            0
        } else if used > h {
            used
        } else {
            let mut count = 0;
            let mut b = self.top.0 as isize - 1;
            let mut i = 0isize;
            while i < scrolled && b < botline as isize {
                b += 1;
                i += if b as usize > last {
                    isize::MAX / 2
                } else {
                    self.plines(t, b as usize, true) as isize
                };
                count += 1;
            }
            if i < scrolled { 9999 } else { count }
        };
        if line_count >= h && line_count > min_scroll {
            self.scroll_cursor_halfway(t, false, true);
        } else if line_count > 0 {
            self.scrollup(t, scrolled as usize);
        }
        if set_topbot {
            self.cursor_correct_sms(t);
        }
    }

    /// Put the cursor line in the middle (scroll_cursor_halfway, the
    /// 'smoothscroll' way: to the exact row). `atend`: as `zz`, also when
    /// that leaves space below the text.
    fn scroll_cursor_halfway(&mut self, t: &dyn TextModel, atend: bool, _prefer_above: bool) {
        let cl = self.cline(t);
        let last = last_line(t);
        let mut used = self.plines(t, cl, true) as isize;
        let (mut loff, mut boff) = (cl as isize, cl);
        let mut topline = cl;
        let mut skip = 0;
        let want_height = if atend {
            let w = (self.height as isize - used) / 2;
            used = 0;
            w
        } else {
            self.height as isize
        };
        while topline > 0 {
            loff -= 1;
            if loff < 0 {
                break;
            }
            let height = self.plines(t, loff as usize, false) as isize;
            used += height;
            if !atend && boff < last {
                boff += 1;
                used += self.plines(t, boff, true) as isize;
            }
            if used > want_height {
                if used - height < want_height {
                    topline = loff as usize;
                    skip = (used - want_height) as usize;
                }
                break;
            }
            topline = loff as usize;
        }
        if self.top.0 != topline || skip != 0 || self.top.1 != 0 {
            self.top = (topline, skip);
        }
    }

    /// Scroll the text up `rows` screen rows (scrollup, CTRL-E); a cursor
    /// left above the view moves to its top line.
    fn scrollup(&mut self, t: &dyn TextModel, rows: usize) {
        let last = last_line(t);
        let w = self.width;
        let mut size = self.line_size(t, self.top.0);
        for _ in 0..rows {
            let mut line = self.top.0;
            self.top.1 += 1;
            if self.top.1 * w >= size {
                if line == last {
                    self.top.1 -= 1;
                    break;
                }
                line += 1;
            }
            if line > self.top.0 {
                self.top = (line, 0);
                size = self.line_size(t, line);
            }
        }
        if self.cline(t) < self.top.0 {
            let want = self.curswant(t);
            self.coladvance(t, self.top.0, want);
        }
    }

    /// Scroll the text down `rows` screen rows (scrolldown, CTRL-Y); a
    /// cursor pushed off the bottom moves up.
    fn scrolldown(&mut self, t: &dyn TextModel, rows: usize) -> bool {
        let w = self.width;
        // The cursor's screen row now (w_wrow).
        let cl = self.cline(t);
        let mut wrow = self.screen_row(t, cl) + (self.virtcol(t) / w) as isize;
        let mut done = 0;
        for _ in 0..rows {
            if self.top.0 == 0 && self.top.1 == 0 {
                break;
            }
            if self.top.1 > 0 {
                self.top.1 -= 1;
            } else {
                self.top.0 -= 1;
                let size = self.line_size(t, self.top.0);
                self.top.1 = if size > w { (size - w - 1) / w + 1 } else { 0 };
            }
            done += 1;
        }
        wrow += done;
        // The row of the cursor line's last row.
        let cheight = self.plines(t, cl, true) as isize;
        let mut end = wrow + cheight - 1 - (self.virtcol(t) / w) as isize;
        let mut line = cl;
        let mut moved = false;
        while end >= self.height as isize && line > 0 {
            end -= self.plines(t, line, true) as isize;
            line -= 1;
            moved = true;
        }
        if moved {
            let want = self.curswant(t);
            self.coladvance(t, line, want);
        }
        if self.cline(t) < self.top.0 {
            let want = self.curswant(t);
            self.coladvance(t, self.top.0, want);
        }
        moved
    }

    /// The screen row `line` starts on (can be negative above the view).
    fn screen_row(&self, t: &dyn TextModel, line: usize) -> isize {
        let (top, skip) = self.top;
        if line >= top {
            let mut row = -(skip as isize);
            for l in top..line {
                row += self.plines(t, l, false) as isize;
            }
            row
        } else {
            let mut row = -(skip as isize);
            for l in line..top {
                row -= self.plines(t, l, false) as isize;
            }
            row
        }
    }

    /// Keep a cursor in the top line out of the rows scrolled off and from
    /// under the marker (cursor_correct_sms).
    fn cursor_correct_sms(&mut self, t: &dyn TextModel) {
        let cl = self.cline(t);
        if !self.scrolling() || cl != self.top.0 {
            return;
        }
        let w = self.width;
        let overlap = if self.top.1 == 0 { 0 } else { MARKER };
        let top = self.top.1 * w + overlap;
        let bot = (self.top.1 + self.height) * w;
        let v = self.virtcol(t);
        let mut col = v;
        if col < top {
            if col < w {
                col += w;
            }
            while col < top {
                col += w;
            }
        } else {
            while col >= bot {
                col -= w;
            }
        }
        if col != v {
            self.want = Some(col);
            let ok = self.coladvance(t, cl, col);
            if !ok
                && self.top.1 > 0
                && cl < last_line(t)
                && self.virtcol(t) < self.top.1 * w + overlap
            {
                // Still hidden: go to the next line instead.
                self.cursor = t.line_to_char(cl + 1);
                self.want = Some(0);
            }
        }
    }

    // ── Scrolling commands ───────────────────────────────────────────────

    /// Scroll the view `rows` screen rows (down if positive), as the mouse
    /// wheel does: as CTRL-E and CTRL-Y, the cursor staying in view.
    pub fn scroll_view(&mut self, t: &dyn TextModel, rows: isize) {
        if self.scrolling() && rows != 0 {
            self.scroll_redraw(t, rows > 0, rows.unsigned_abs());
        }
    }

    /// CTRL-E and friends.
    pub(super) fn scroll(&mut self, t: &dyn TextModel, s: Scroll, count: Option<usize>) -> R {
        if !self.scrolling() {
            return Err(Beep);
        }
        match s {
            Scroll::Line(up) => {
                self.scroll_redraw(t, up, count.unwrap_or(1));
                Ok(())
            }
            Scroll::Half(down) => self.pagescroll(t, down, count, true),
            Scroll::Page(down) => self.pagescroll(t, down, Some(count.unwrap_or(1)), false),
        }
    }

    /// Scroll `count` rows and keep the cursor in view (scroll_redraw).
    fn scroll_redraw(&mut self, t: &dyn TextModel, up: bool, count: usize) {
        let prev = self.cline(t);
        if up {
            self.scrollup(t, count);
        } else {
            self.scrolldown(t, count);
        }
        self.cursor_correct_sms(t);
        let cl = self.cline(t);
        if cl != prev {
            let want = self.curswant(t);
            self.coladvance(t, cl, want);
        }
    }

    /// CTRL-D, CTRL-U (`half`), CTRL-F and CTRL-B (pagescroll).
    fn pagescroll(&mut self, t: &dyn TextModel, down: bool, count: Option<usize>, half: bool) -> R {
        let h = self.height;
        let n_lines = last_line(t) + 1;
        let prev_cursor = self.cursor;
        let prev_top = self.top;
        let prev_want = self.want;
        if half {
            // A count sets the 'scroll' option, as in Vim.
            if let Some(c) = count {
                self.scroll_lines = Some(c.min(h));
            }
            let mut count = self.scroll_lines.unwrap_or(h / 2).min(h);
            let curscount = count;
            // Don't scroll past the end of the text.
            if down && self.top.0 + 1 + h + count > n_lines {
                let mut n = self.plines_correct_topline(t, self.top.0, false);
                if n < h + count && self.top.0 < n_lines - 1 {
                    let mut sum = 0;
                    for l in self.top.0 + 1..n_lines {
                        if sum >= h + count {
                            break;
                        }
                        sum += self.plines(t, l, false);
                    }
                    n += sum.min(h + count);
                }
                if n < h + count {
                    count = n.saturating_sub(h);
                }
            }
            if count > 0 {
                self.scroll_redraw(t, down, count);
                self.cursor = prev_cursor;
                self.want = prev_want;
            }
            let dir = if down { Dir::Forward } else { Dir::Backward };
            let (to, _) = self.screengo(t, dir, curscount);
            self.cursor = to;
        } else {
            let overlap = self.scroll_overlap(t, down);
            let count = count.unwrap_or(1) * overlap;
            self.scroll_redraw(t, down, count);
            if self.top != prev_top {
                let (botline, _) = self.botline(t);
                let line = if down {
                    self.top.0
                } else {
                    botline.saturating_sub(1)
                };
                let col = text::line_col(t, self.cursor).1;
                self.cursor = text::pos(t, line, col.min(line_len(t, line)));
                self.clamp(t);
            }
        }
        if self.top == prev_top && self.cursor == prev_cursor {
            return Err(Beep);
        }
        Ok(())
    }

    /// How far CTRL-F and CTRL-B go: a window less up to two lines of
    /// overlap (get_scroll_overlap).
    fn scroll_overlap(&self, t: &dyn TextModel, down: bool) -> usize {
        let h = self.height;
        let min_height = h.saturating_sub(2);
        let (botline, _) = self.botline(t);
        let last = last_line(t);
        if (!down && self.top.0 == 0) || (down && botline > last) {
            return min_height + 2;
        }
        let height = |l: isize| -> usize {
            if l < 0 || l as usize > last {
                usize::MAX / 4
            } else {
                self.plines(t, l as usize, true)
            }
        };
        let step: isize = if down { -1 } else { 1 };
        let mut l = if down {
            botline as isize
        } else {
            self.top.0 as isize - 1
        };
        let h1 = height(l);
        if h1 > min_height {
            return min_height + 2;
        }
        l += step;
        let h2 = height(l);
        if h2 + h1 > min_height {
            return min_height + 2;
        }
        l += step;
        let h3 = height(l);
        if h3 + h2 > min_height {
            return min_height + 2;
        }
        l += step;
        let h4 = height(l);
        if h4 + h3 + h2 > min_height || h3 + h2 + h1 > min_height {
            min_height + 1
        } else {
            min_height
        }
    }

    // ── Moving by screen line ────────────────────────────────────────────

    /// `gj`/`gk`: `dist` screen rows down or up (nv_screengo). The place,
    /// and whether it got all the way.
    pub(super) fn screengo(&mut self, t: &dyn TextModel, dir: Dir, dist: usize) -> (Pos, bool) {
        let w = self.width.max(1);
        let mut cl = self.cline(t);
        let last = last_line(t);
        let mut linelen = self.line_size(t, cl);
        let mut ok = true;
        let mut atend = false;
        let span = |len: usize| {
            if len > w {
                ((len - w - 1) / w + 1) * w + w
            } else {
                w
            }
        };
        let mut want = if self.want == Some(usize::MAX) {
            atend = true;
            let v = self.virtcol(t);
            let mut want = w - 1;
            if v > want {
                want += ((v - want - 1) / w + 1) * w;
            }
            want
        } else {
            self.curswant(t).min(span(linelen) - 1)
        };
        for _ in 0..dist {
            if dir == Dir::Backward {
                if want >= w {
                    want -= w;
                } else {
                    if cl == 0 {
                        ok = false;
                        break;
                    }
                    cl -= 1;
                    linelen = self.line_size(t, cl);
                    if linelen > w {
                        want += ((linelen - w - 1) / w + 1) * w;
                    }
                }
            } else if want + w < span(linelen) {
                want += w;
            } else {
                if cl >= last {
                    ok = false;
                    break;
                }
                cl += 1;
                want %= w;
                linelen = self.line_size(t, cl);
            }
        }
        self.coladvance(t, cl, want);
        let col = text::line_col(t, self.cursor).1;
        if col > 0 {
            // Landed on a char split over the row's end: back to this row.
            let v = self.virtcol(t);
            let over_half = if want < w {
                want > w / 2
            } else {
                (want - w) % w > w / 2
            };
            if v > want && over_half {
                self.cursor -= 1;
            }
        }
        self.want = Some(if atend { usize::MAX } else { want });
        self.adjust_skipcol(t);
        (self.cursor, ok)
    }

    /// `g0`, `g^`, `gm`: the screen row's start, first non-blank, middle.
    pub(super) fn screen_home(&mut self, t: &dyn TextModel, which: char) -> Pos {
        let w = self.width.max(1);
        let cl = self.cline(t);
        let v = self.virtcol(t);
        let mut i = if v >= w { (v - w) / w * w + w } else { 0 };
        if self.top.1 > 0 && cl == self.top.0 && i == self.top.1 * w {
            i += MARKER;
        }
        if which == 'm' {
            i += w / 2;
        }
        self.coladvance(t, cl, i);
        if which == '^' {
            let len = line_len(t, cl);
            let (_, mut col) = text::line_col(t, self.cursor);
            while col + 1 < len && text::char_at(t, cl, col).is_some_and(is_blank) {
                col += 1;
            }
            self.cursor = text::pos(t, cl, col);
        }
        self.want = None;
        self.adjust_skipcol(t);
        self.cursor
    }

    /// `g$`: the screen row's last char.
    pub(super) fn screen_end(&mut self, t: &dyn TextModel, count: usize) -> R<Pos> {
        let w = self.width.max(1);
        if count > 1 {
            self.want = Some(usize::MAX);
            let (to, ok) = self.screengo(t, Dir::Forward, count - 1);
            return if ok { Ok(to) } else { Err(Beep) };
        }
        let cl = self.cline(t);
        let v = self.virtcol(t);
        let mut i = w - 1;
        if v >= w {
            i += ((v - w) / w + 1) * w;
        }
        self.coladvance(t, cl, i);
        // It sticks to this column, even if the char was split over rows.
        let v = self.virtcol(t);
        self.want = Some(v);
        let col = text::line_col(t, self.cursor).1;
        if col > 0 && v > i {
            self.cursor -= 1;
        }
        Ok(self.cursor)
    }

    /// `H`, `M`, `L`: the top, middle or bottom line on screen.
    pub(super) fn screen_line(
        &mut self,
        t: &dyn TextModel,
        which: char,
        count: usize,
        op: bool,
    ) -> Pos {
        let last = last_line(t);
        let line = match which {
            'L' => {
                let (botline, _) = self.botline(t);
                let l = botline.saturating_sub(1);
                l.saturating_sub(count - 1)
            }
            'M' => {
                let (_, empty) = self.botline(t);
                let half = (self.height - empty).div_ceil(2);
                let mut used = 0;
                let mut n = 0;
                while self.top.0 + n < last {
                    used += self.plines(t, self.top.0 + n, true);
                    if used >= half {
                        break;
                    }
                    n += 1;
                }
                if n > 0 && used > self.height {
                    n -= 1;
                }
                (self.top.0 + n).min(last)
            }
            _ => (self.top.0 + count - 1).min(last),
        };
        let want = self.curswant(t);
        self.cursor = t.line_to_char(line);
        if !op {
            self.cursor_correct(t);
        }
        let line = self.cline(t);
        self.coladvance(t, line, want);
        self.cursor
    }

    /// Keep the cursor on a line wholly on screen, for `H`, `M` and `L`
    /// (cursor_correct, with 'scrolloff' 0).
    fn cursor_correct(&mut self, t: &dyn TextModel) {
        let (botline, _) = self.botline(t);
        let cln = self.cline(t);
        let top = self.top.0;
        if cln >= top && cln < botline {
            return;
        }
        let bot = botline as isize - 1;
        let line = if top as isize == bot || bot < 0 {
            Some(top)
        } else if top as isize > bot {
            Some(bot as usize)
        } else if cln < top && top > 0 {
            Some(top)
        } else if cln as isize > bot && botline <= last_line(t) {
            Some(bot as usize)
        } else {
            None
        };
        if let Some(l) = line {
            let col = text::line_col(t, self.cursor).1;
            self.cursor = text::pos(t, l, col.min(line_len(t, l)));
        }
    }

    /// `zt`, `z<CR>`, `zz`, `z.`, `zb`, `z-`, `z+`, `z^`; a count is the
    /// line to put there.
    pub(super) fn z(&mut self, t: &dyn TextModel, c: char, count: Option<usize>) -> R {
        if !self.scrolling() {
            return Err(Beep);
        }
        let last = last_line(t);
        if let Some(n) = count
            && "+\rtz.^-b".contains(c)
            && n.saturating_sub(1) != self.cline(t)
        {
            // Going to another line is a jump.
            self.setpcmark(t);
            let line = n.saturating_sub(1).min(last);
            let col = text::line_col(t, self.cursor).1;
            self.cursor = text::pos(t, line, col.min(line_len(t, line)));
            self.clamp(t);
        }
        let first_non_blank = |s: &mut Self| {
            let l = s.cline(t);
            s.cursor = text::pos(
                t,
                l,
                text::first_non_blank(t, l).min(line_len(t, l).saturating_sub(1)),
            );
            s.want = None;
        };
        match c {
            '+' | '\r' | 't' => {
                if c == '+' && count.is_none() {
                    let (botline, _) = self.botline(t);
                    self.cursor = t.line_to_char(botline.min(last));
                }
                if c != 't' {
                    first_non_blank(self);
                }
                self.scroll_cursor_top(t, 0, true);
            }
            '.' | 'z' => {
                if c == '.' {
                    first_non_blank(self);
                }
                self.scroll_cursor_halfway(t, true, false);
            }
            '^' | '-' | 'b' => {
                if c == '^' {
                    let line = if count.is_some() {
                        self.scroll_cursor_bot(t, 0, true);
                        self.top.0
                    } else {
                        self.top.0.saturating_sub(1)
                    };
                    self.cursor = t.line_to_char(line);
                }
                if c != 'b' {
                    first_non_blank(self);
                }
                self.scroll_cursor_bot(t, 0, true);
            }
            _ => return Err(Beep),
        }
        Ok(())
    }
}
