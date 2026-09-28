//! CTRL-A and CTRL-X: add to or subtract from the number at or after the
//! cursor, as Neovim's do_addsub() does with its default 'nrformats'
//! ("bin,hex": decimal, `0x` hex and `0b` binary; no octal, no letters).
//! In visual mode, the first number in the selection on each line, and
//! with `g` a growing amount (1, 2, 3... on the lines changed).

use super::{Beep, R, Vim};
use crate::TextModel;
use crate::text::{self, line_len, line_text};

/// A number changed on a line: `len` chars from `col` became `with`.
struct Change {
    col: usize,
    len: usize,
    with: String,
}

fn is_bdigit(c: char) -> bool {
    c == '0' || c == '1'
}

/// Vim's vim_str2nr() with bin and hex: the prefix ('x', 'X', 'b', 'B', or
/// none for decimal), how many chars the number takes (a `-` too), its
/// value, and whether it was too big.
fn str2nr(s: &[char], maxlen: usize) -> (Option<char>, usize, u64, bool) {
    let ended = |i: usize| maxlen != 0 && i >= maxlen;
    let at = |i: usize| s.get(i).copied().unwrap_or('\0');
    let mut i = usize::from(at(0) == '-');
    let mut pre = None;
    let base: u64 = if !ended(i + 1) && at(i) == '0' && at(i + 1) != '8' && at(i + 1) != '9' {
        let p = at(i + 1);
        if !ended(i + 2) && matches!(p, 'x' | 'X') && at(i + 2).is_ascii_hexdigit() {
            pre = Some(p);
            i += 2;
            16
        } else if !ended(i + 2) && matches!(p, 'b' | 'B') && is_bdigit(at(i + 2)) {
            pre = Some(p);
            i += 2;
            2
        } else {
            10
        }
    } else {
        10
    };
    let mut n: u64 = 0;
    let mut overflow = false;
    while !ended(i) {
        let Some(d) = at(i).to_digit(base as u32) else {
            break;
        };
        let d = u64::from(d);
        if n < u64::MAX / base || (n == u64::MAX / base && (base != 10 || d <= u64::MAX % 10)) {
            n = base * n + d;
        } else {
            n = u64::MAX;
            overflow = true;
        }
        i += 1;
    }
    (pre, i, n, overflow)
}

/// do_addsub() on one line: at the cursor's column, or in visual mode
/// within `length` chars from `col` (`maxlen` limiting the number read).
/// `hexupper` is Vim's static: the case of the last hex number's letters.
fn addsub_line(
    line: &[char],
    cursor: usize,
    visual: Option<(usize, usize)>,
    sub: bool,
    amount: u64,
    hexupper: &mut bool,
) -> Result<Option<Change>, Beep> {
    let at = |i: usize| line.get(i).copied().unwrap_or('\0');
    let linelen = line.len();
    let mut col = cursor;
    if col >= linelen {
        return Ok(None);
    }
    let mut negative = false;
    let mut was_positive = true;
    let mut maxlen = 0;
    match visual {
        None => {
            // On a hex or binary number, after its "0x": back to its start.
            while col > 0 && is_bdigit(at(col)) {
                col -= 1;
            }
            while col > 0 && at(col).is_ascii_hexdigit() {
                col -= 1;
            }
            let hex_at = |c: usize| {
                c > 0
                    && matches!(at(c), 'x' | 'X')
                    && at(c - 1) == '0'
                    && at(c + 1).is_ascii_hexdigit()
            };
            if !hex_at(col) {
                // (Binary and hex overlap: look again.)
                col = cursor;
                while col > 0 && at(col).is_ascii_digit() {
                    col -= 1;
                }
            }
            let bin_at = |c: usize| {
                c > 0 && matches!(at(c), 'b' | 'B') && at(c - 1) == '0' && is_bdigit(at(c + 1))
            };
            if hex_at(col) || bin_at(col) {
                col -= 1;
            } else {
                // The number at or after the cursor, from its start.
                col = cursor;
                while col < linelen && !at(col).is_ascii_digit() {
                    col += 1;
                }
                while col > 0 && at(col - 1).is_ascii_digit() {
                    col -= 1;
                }
            }
        }
        Some((mut length, maxlen_v)) => {
            while col < linelen && length > 0 && !at(col).is_ascii_digit() {
                col += 1;
                length -= 1;
            }
            if length == 0 {
                return Ok(None);
            }
            if col > cursor && at(col - 1) == '-' {
                negative = true;
                was_positive = false;
            }
            // (Read no further than the selection, unless it's to the end.)
            maxlen = if maxlen_v == usize::MAX {
                usize::MAX
            } else {
                length
            };
        }
    }
    let firstdigit = at(col);
    if !firstdigit.is_ascii_digit() {
        return Err(Beep);
    }
    if visual.is_none() && col > 0 && at(col - 1) == '-' {
        col -= 1;
        negative = true;
    }
    let (pre, mut length, mut n, overflow) =
        str2nr(&line[col..], if maxlen == usize::MAX { 0 } else { maxlen });
    if pre.is_some() && negative {
        // A '-' before a hex or binary number isn't a sign.
        col += 1;
        length -= 1;
        negative = false;
    }
    let subtract = sub ^ negative;
    let oldn = n;
    if !overflow {
        n = if subtract {
            n.wrapping_sub(amount)
        } else {
            n.wrapping_add(amount)
        };
    }
    // Decimal numbers go through zero to negative, and back.
    if pre.is_none() {
        if subtract {
            if n > oldn {
                n = n.wrapping_neg();
                negative ^= true;
            }
        } else if n < oldn {
            n = !n;
            negative ^= true;
        }
        if n == 0 {
            negative = false;
        }
    }
    if visual.is_some() && !was_positive && !negative && col > 0 {
        // The '-' goes.
        col -= 1;
        length += 1;
    }
    let old = &line[col..(col + length).min(linelen)];
    if old.first() == Some(&'-') {
        length -= 1;
    }
    if let Some(c) = old.iter().rev().find(|c| c.is_ascii_alphabetic()) {
        *hexupper = c.is_ascii_uppercase();
    }
    let mut with = String::new();
    if negative && (visual.is_none() || was_positive) {
        with.push('-');
    }
    if let Some(p) = pre {
        with.push('0');
        with.push(p);
        length = length.saturating_sub(2);
    }
    let digits = match pre {
        Some('b' | 'B') => {
            if n == 0 {
                String::new()
            } else {
                format!("{n:b}")
            }
        }
        Some(_) if *hexupper => format!("{n:X}"),
        Some(_) => format!("{n:x}"),
        None => n.to_string(),
    };
    // Leading zeros: as many as keep the number's length.
    if firstdigit == '0' {
        for _ in 0..length.saturating_sub(digits.len()) {
            with.push('0');
        }
    }
    with.push_str(&digits);
    Ok(Some(Change {
        col,
        len: old.len(),
        with,
    }))
}

impl Vim {
    /// CTRL-A (`sub` false) and CTRL-X in normal mode, `count` as the amount.
    pub(super) fn add_sub(&mut self, t: &mut dyn TextModel, sub: bool, count: usize) -> R {
        let (line, col) = self.lc(t);
        let chars: Vec<char> = line_text(t, line).chars().collect();
        let mut upper = self.hexupper;
        let change = addsub_line(&chars, col, None, sub, count as u64, &mut upper)?;
        self.hexupper = upper;
        let Some(c) = change else {
            return Ok(());
        };
        self.begin_group();
        let from = text::pos(t, line, c.col);
        self.edit_hint = Some(super::EditHint::Spanned);
        self.edit(t, from..from + c.len, &c.with);
        let end = c.col + c.with.chars().count();
        self.marks.op_start = Some(self.mk(t, (line, c.col)));
        self.marks.op_end = Some(self.mk(t, (line, end.saturating_sub(1))));
        // (Vim's changed_lines() for the line: '. at its start.)
        self.marks.last_change = Some((line, 0));
        self.cursor = text::pos(t, line, end.saturating_sub(1));
        self.want = None;
        Ok(())
    }

    /// CTRL-A and CTRL-X on a visual selection (`g`: `progressive`): lines
    /// `first..=last`, from `start_col` on the first and to `end_col` on the
    /// last (all of each line for a line selection, `lines`). `to_end`: the
    /// selection went to the lines' ends (`$`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn add_sub_visual(
        &mut self,
        t: &mut dyn TextModel,
        sub: bool,
        count: usize,
        progressive: bool,
        (first, start_col): (usize, usize),
        (last, end_col): (usize, usize),
        lines: bool,
        to_end: bool,
    ) -> R {
        self.begin_group();
        if let Some(g) = self.group.as_mut() {
            g.top = Some(g.top.map_or(first, |l| l.min(first)));
        }
        let mut amount = count as u64;
        let mut first_change = None;
        let mut last_end = None;
        for line in first..=last {
            let len = line_len(t, line);
            let (col, length) = if lines {
                (0, len)
            } else {
                let col = if line == first { start_col } else { 0 };
                let length = if line == last {
                    end_col.min(len.saturating_sub(1)) as isize - col as isize + 1
                } else {
                    len as isize - col as isize
                };
                (col, length.max(0) as usize)
            };
            let maxlen = if lines || to_end { usize::MAX } else { length };
            let chars: Vec<char> = line_text(t, line).chars().collect();
            let mut upper = self.hexupper;
            // (A beep here doesn't stop the other lines.)
            let change = addsub_line(&chars, col, Some((length, maxlen)), sub, amount, &mut upper);
            self.hexupper = upper;
            let Ok(Some(c)) = change else {
                continue;
            };
            let from = text::pos(t, line, c.col);
            self.edit_hint = Some(super::EditHint::Spanned);
            self.edit(t, from..from + c.len, &c.with);
            let end = c.col + c.with.chars().count();
            first_change.get_or_insert((line, c.col));
            last_end = Some((line, end.saturating_sub(1)));
            if progressive {
                amount += count as u64;
            }
        }
        if let (Some(s), Some(e)) = (first_change, last_end) {
            self.marks.op_start = Some(self.mk(t, s));
            self.marks.op_end = Some(self.mk(t, e));
            self.marks.last_change = Some((first, 0));
        }
        Ok(())
    }
}
