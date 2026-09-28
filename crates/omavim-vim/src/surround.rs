//! Omavim's own Markdown keys: bold and italic around the word or the
//! selection (or taken off again), and a link around it.

use super::{Beep, Insert, R, Vim};
use crate::motion::Cur;
use crate::text::{self, is_blank, line_len};
use crate::{Mode, Pos, TextModel};
use std::ops::Range;

impl Vim {
    /// Put `mark` either side of the word under the cursor, or the visual
    /// selection, or take it off if it's there; on a blank, type between a
    /// pair. One undo step, back in normal mode.
    pub fn surround(&mut self, t: &mut dyn TextModel, mark: &str) -> R {
        let Some(r) = self.target(t)? else {
            return self.type_between(t, mark, mark);
        };
        let n = mark.chars().count();
        let has = |r: &Range<Pos>| {
            r.start >= n
                && r.end + n <= t.len_chars()
                && t.slice(r.start - n..r.start) == mark
                && t.slice(r.end..r.end + n) == mark
        };
        let inside = r.end - r.start > 2 * n
            && t.slice(r.start..r.start + n) == mark
            && t.slice(r.end - n..r.end) == mark;
        self.begin_group();
        if has(&r) {
            self.edit(t, r.end..r.end + n, "");
            self.edit(t, r.start - n..r.start, "");
            self.cursor = r.start - n;
        } else if inside {
            self.edit(t, r.end - n..r.end, "");
            self.edit(t, r.start..r.start + n, "");
            self.cursor = r.start;
        } else {
            self.edit(t, r.end..r.end, mark);
            self.edit(t, r.start..r.start, mark);
            self.cursor = r.start;
        }
        self.close_group();
        Ok(())
    }

    /// `[text](...)` around the word or the selection, typing the address.
    pub fn link(&mut self, t: &mut dyn TextModel) -> R {
        let Some(r) = self.target(t)? else {
            return self.type_between(t, "[", "]()");
        };
        self.begin_group();
        self.edit(t, r.end..r.end, "]()");
        self.edit(t, r.start..r.start, "[");
        self.cursor = r.end + 3;
        self.start_insert(t, Insert::Before, 1)
    }

    /// What to wrap: the selection (leaving visual mode), or the word under
    /// the cursor (None on a blank or an empty line).
    fn target(&mut self, t: &dyn TextModel) -> Result<Option<Range<Pos>>, Beep> {
        match self.mode {
            Mode::Visual | Mode::VisualLine => {
                let (a, b) = (self.anchor.min(self.cursor), self.anchor.max(self.cursor));
                let (al, _) = text::line_col(t, a);
                let (bl, _) = text::line_col(t, b);
                let r = if self.mode == Mode::VisualLine {
                    t.line_to_char(al)..t.line_to_char(bl) + line_len(t, bl)
                } else {
                    // Not the line break, when the selection ends on one.
                    a..(b + 1).min(t.line_to_char(bl) + line_len(t, bl)).max(a)
                };
                self.mode = Mode::Normal;
                Ok((r.end > r.start).then_some(r))
            }
            Mode::Normal => {
                let (line, col) = text::line_col(t, self.cursor);
                if col >= line_len(t, line) || is_blank(t.char(self.cursor)) {
                    return Ok(None);
                }
                let o = crate::textobj::word(t, Cur::new(line, col), None, 1, false, false)
                    .map_err(|_| Beep)?;
                let start = text::pos(t, o.start.line, o.start.col);
                let end = text::pos(t, o.end.line, o.end.col) + usize::from(o.inclusive);
                Ok(Some(start..end))
            }
            _ => Err(Beep),
        }
    }

    /// A pair of marks with insert mode between them.
    fn type_between(&mut self, t: &mut dyn TextModel, open: &str, close: &str) -> R {
        self.begin_group();
        let at = self.cursor.min(t.len_chars());
        let (line, col) = text::line_col(t, at);
        // After the char under the cursor, as `a`, unless the line's empty.
        let at = if col < line_len(t, line) { at + 1 } else { at };
        self.edit(t, at..at, &format!("{open}{close}"));
        self.cursor = at + open.chars().count();
        self.start_insert(t, Insert::Before, 1)
    }
}
