//! `gq` and `gw`: format lines, as Neovim's format_lines() and
//! internal_format() do with its defaults ('formatoptions' "tcqj", the
//! default 'comments', 'autoindent', 'noexpandtab', 'textwidth' 0 so the
//! window's width less one, at most 79). Each paragraph's lines are joined,
//! then broken at blanks to fit; a comment leader (`//`, `#`, `>`, ...) is
//! kept at the start of each of its lines.

use super::{EditHint, Vim};
use crate::TextModel;
use crate::text::{self, is_blank, last_line, line_len, line_text};

/// Neovim's default 'comments': (flags, string) for each part.
const COMMENTS: [(&str, &str); 9] = [
    ("s1", "/*"),
    ("mb", "*"),
    ("ex", "*/"),
    ("", "//"),
    ("b", "#"),
    ("", "%"),
    ("", "XCOMM"),
    ("n", ">"),
    ("fb", "-"),
];

fn white(c: Option<&char>) -> bool {
    c.is_some_and(|&c| c == ' ' || c == '\t')
}

/// The comment leader a line starts with (Vim's get_leader_len, with the
/// blanks after it): its length in chars, and the 'comments' part.
fn leader_len(line: &[char]) -> (usize, Option<usize>) {
    let mut i = 0;
    while white(line.get(i)) {
        i += 1;
    }
    let mut result = 0;
    let mut part = None;
    let mut got_com = false;
    while i < line.len() {
        let mut found = false;
        let mut middle: Option<(usize, usize)> = None;
        for (p, &(flags, string)) in COMMENTS.iter().enumerate() {
            if middle.is_some() && !flags.contains('m') && !flags.contains('e') {
                break;
            }
            if got_com && !flags.contains('n') {
                continue;
            }
            let s: Vec<char> = string.chars().collect();
            if line.get(i..i + s.len()) != Some(&s[..]) {
                continue;
            }
            if flags.contains('b') && !white(line.get(i + s.len())) && i + s.len() < line.len() {
                continue;
            }
            if flags.contains('m') {
                if middle.is_none() {
                    middle = Some((s.len(), p));
                }
                continue;
            }
            if middle.is_some_and(|(len, _)| s.len() > len) {
                middle = None;
            }
            if middle.is_none() {
                i += s.len();
                if !got_com {
                    part = Some(p);
                }
            }
            found = true;
            break;
        }
        if let Some((len, p)) = middle {
            if !got_com {
                part = Some(p);
            }
            i += len;
            found = true;
        }
        if !found {
            break;
        }
        while white(line.get(i)) {
            i += 1;
        }
        result = i;
        got_com = true;
        if !part.is_some_and(|p| COMMENTS[p].0.contains('n')) {
            break;
        }
    }
    (result, if result > 0 { part } else { None })
}

/// Not part of a paragraph (Vim's fmt_check_par): blank, only a comment
/// leader, the end of a comment, or a section start. With the leader.
fn check_par(line: &[char]) -> (bool, usize, Option<usize>) {
    let (len, part) = leader_len(line);
    let ends = part.is_some_and(|p| COMMENTS[p].0.contains('e'));
    let blank = line[len..].iter().all(|&c| c == ' ' || c == '\t');
    (
        blank || (len > 0 && ends) || text::starts_paragraph(line),
        len,
        part,
    )
}

/// Whether two lines' leaders are the same, so they join (same_leader).
fn same_leader(
    line1: &[char],
    len1: usize,
    part1: Option<usize>,
    line2: &[char],
    len2: usize,
    part2: Option<usize>,
) -> bool {
    if len1 == 0 {
        return len2 == 0;
    }
    if let Some(p) = part1 {
        let flags = COMMENTS[p].0;
        if flags.contains('f') {
            return len2 == 0;
        }
        if flags.contains('e') {
            return false;
        }
        if flags.contains('s') {
            if line1.len() <= len1 || len2 == 0 {
                return false;
            }
            return part2.is_some_and(|p| COMMENTS[p].0.contains('m'));
        }
    }
    let mut i1 = 0;
    while white(line1.get(i1)) {
        i1 += 1;
    }
    let mut i2 = 0;
    while i2 < len2 {
        if !white(line2.get(i2)) {
            if line1.get(i1) != line2.get(i2) {
                break;
            }
            i1 += 1;
        } else {
            while white(line1.get(i1)) {
                i1 += 1;
            }
        }
        i2 += 1;
    }
    i2 == len2 && i1 == len1
}

impl Vim {
    /// The width `gq` formats to: the window's, less one, at most 79.
    fn text_width(&self) -> usize {
        if self.width == 0 {
            79
        } else {
            (self.width - 1).min(79)
        }
    }

    /// Indent of `cols` columns as Vim makes it: tabs, then spaces.
    fn indent_string(&self, cols: usize) -> String {
        let ts = self.tabstop.max(1);
        format!("{}{}", "\t".repeat(cols / ts), " ".repeat(cols % ts))
    }

    fn indent_cols(&self, line: &[char]) -> usize {
        let mut v = 0;
        for &c in line {
            match c {
                ' ' => v += 1,
                '\t' => v += self.tabstop - v % self.tabstop,
                _ => break,
            }
        }
        v
    }

    /// Replace a line's indent with Vim's form of it (set_indent).
    fn set_indent(&mut self, t: &mut dyn TextModel, line: usize, cols: usize) {
        let chars: Vec<char> = line_text(t, line).chars().collect();
        let old = chars.iter().take_while(|&&c| c == ' ' || c == '\t').count();
        let new = self.indent_string(cols);
        if chars[..old].iter().collect::<String>() != new {
            let start = t.line_to_char(line);
            self.edit(t, start..start + old, &new);
            // `gw`'s cursor stays on its text (and set_indent moves only it).
            if let Some((l, c)) = self.marks.saved.as_mut()
                && *l == line
            {
                let (old_b, new_b) = (old, new.len());
                if *c >= old_b {
                    *c = *c + new_b - old_b;
                } else if *c >= new_b {
                    *c = new_b;
                }
            }
        }
    }

    /// `gq` (`keep`: `gw`) over `count` lines from `first`: what Neovim's
    /// op_format does. The cursor ends on the last line's first non-blank
    /// (`gw`: where it was); '[ and '] around what was formatted.
    pub(super) fn format(
        &mut self,
        t: &mut dyn TextModel,
        first: usize,
        count: usize,
        keep: bool,
        end_adjusted: bool,
    ) {
        // Where the command was typed: undo comes back there, and `gw`.
        let start = self.cmd_start.min(t.len_chars());
        self.begin_group();
        self.set_undo_cursor(start);
        self.marks.op_start = Some((first, 0));
        if keep {
            self.marks.saved = Some(self.mk(t, text::line_col(t, start)));
        }
        let edits = self.group.as_ref().map_or(0, |g| g.edits.len());
        let mut last = self.format_lines(t, first, count);
        // Nothing to change is still an undo step (Vim saved the lines).
        if self.group.as_ref().map_or(0, |g| g.edits.len()) == edits {
            self.noop_undo_step(t, t.line_to_char(first));
        }
        // A motion's end moved back a line (`gq}`): on to the next, so `.`
        // goes on from there.
        if end_adjusted && last < last_line(t) {
            last += 1;
        }
        let line = last.min(last_line(t));
        self.go_line(t, line);
        self.marks.op_end = Some(self.mk(t, text::line_col(t, self.cursor)));
        if keep && let Some(m) = self.marks.saved.take() {
            let (l, c) = self.unmk(t, m);
            let l = l.min(last_line(t));
            self.cursor = text::pos(t, l, c.min(line_len(t, l).saturating_sub(1)));
        }
        self.want = None;
    }

    /// Neovim's format_lines(): the line the cursor ends on.
    fn format_lines(&mut self, t: &mut dyn TextModel, first: usize, count: usize) -> usize {
        let chars =
            |t: &dyn TextModel, l: usize| -> Vec<char> { line_text(t, l).chars().collect() };
        let max_len = self.text_width() * 3;
        let mut line = first;
        let (mut is_not_par, mut leader, mut part) = if first > 0 {
            check_par(&chars(t, first - 1))
        } else {
            (true, 0, None)
        };
        let (mut next_is_not_par, mut next_leader, mut next_part) = check_par(&chars(t, first));
        let mut advance = true;
        let mut need_set_indent = true;
        let mut force_format = false;
        let mut first_iteration = true;
        let mut remaining = count;
        while remaining > 0 {
            if advance {
                if !first_iteration {
                    line += 1;
                }
                is_not_par = next_is_not_par;
                leader = next_leader;
                part = next_part;
            }
            first_iteration = false;
            if remaining == 1 || line >= last_line(t) {
                next_is_not_par = true;
                next_leader = 0;
                next_part = None;
            } else {
                (next_is_not_par, next_leader, next_part) = check_par(&chars(t, line + 1));
            }
            advance = true;
            let mut is_end_par = is_not_par || next_is_not_par;
            if !is_not_par {
                // A change of comment leader ends the paragraph.
                if line >= last_line(t)
                    || !same_leader(
                        &chars(t, line),
                        leader,
                        part,
                        &chars(t, line + 1),
                        next_leader,
                        next_part,
                    )
                {
                    is_end_par = true;
                }
                if is_end_par || force_format {
                    if need_set_indent {
                        let indent = self.indent_cols(&chars(t, line));
                        self.set_indent(t, line, indent);
                    }
                    // Break it up, from its last non-blank.
                    let text = chars(t, line);
                    let mut col = text.len().saturating_sub(1);
                    while col > 0 && is_blank(text[col]) {
                        col -= 1;
                    }
                    line = self.internal_format(t, line, col);
                    need_set_indent = is_end_par;
                    force_format = false;
                }
                if !is_end_par {
                    // Same paragraph: join the next line on, without its
                    // leader.
                    advance = false;
                    if next_leader > 0 {
                        let start = t.line_to_char(line + 1);
                        let bytes = chars(t, line + 1)[..next_leader]
                            .iter()
                            .map(|c| c.len_utf8())
                            .sum();
                        self.edit_hint = Some(EditHint::ColShift(line + 1, bytes));
                        self.edit(t, start..start + next_leader, "");
                    }
                    self.join_next(t, line, true);
                    force_format = line_len(t, line) > max_len;
                }
            }
            remaining -= 1;
        }
        line
    }

    /// Neovim's internal_format() for `gq`: break line `line` at blanks
    /// until it fits, from the cursor at `col`. The line it ends on.
    fn internal_format(&mut self, t: &mut dyn TextModel, mut line: usize, mut col: usize) -> usize {
        let tw = self.text_width();
        let ts = self.tabstop;
        let mut no_leader = false;
        loop {
            let chars: Vec<char> = line_text(t, line).chars().collect();
            let at = |i: usize| chars.get(i).copied();
            let width =
                at(col).map_or(1, |c| text::char_width(c, text::vcol(t, line, col, ts), ts));
            if text::vcol(t, line, col, ts) + width <= tw {
                break;
            }
            let leader = if no_leader { 0 } else { leader_len(&chars).0 };
            if leader == 0 {
                no_leader = true;
            }
            let startcol = col;
            if startcol == 0 {
                break;
            }
            let wantcol = text::col_at_vcol(t, line, tw, ts);
            let mut cur = startcol;
            let mut foundcol = 0;
            loop {
                let cc = at(cur);
                if white(cc.as_ref()) {
                    let mut c = cc;
                    while cur > 0 && white(c.as_ref()) {
                        cur -= 1;
                        c = at(cur);
                    }
                    if cur == 0 && white(c.as_ref()) {
                        break;
                    }
                    if cur < leader {
                        break;
                    }
                    cur += 1;
                    foundcol = cur;
                    if cur <= wantcol {
                        break;
                    }
                }
                if cur == 0 {
                    break;
                }
                cur -= 1;
            }
            if foundcol == 0 {
                break;
            }
            // The text after the blanks goes to a new line.
            let mut extra = foundcol;
            while white(at(extra).as_ref()) {
                extra += 1;
            }
            let rel = startcol.saturating_sub(extra);
            let (prefix, _) = self.open_line_prefix(&chars, foundcol);
            let bytes = |n: usize| chars[..n].iter().map(|c| c.len_utf8()).sum::<usize>();
            let delta = prefix.len() as isize - bytes(extra) as isize;
            let from = t.line_to_char(line) + foundcol;
            let to = t.line_to_char(line) + extra;
            self.edit_hint = Some(EditHint::Split {
                line,
                col: bytes(extra),
                delta,
            });
            self.edit(t, from..to, &format!("\n{prefix}"));
            line += 1;
            col = (prefix.chars().count() + rel).min(line_len(t, line));
        }
        line
    }

    /// What open_line() starts the new line with when `gq` breaks `line`
    /// at `col`: its comment leader (as the next line's), or its indent.
    /// And the leader's length in it.
    fn open_line_prefix(&self, line: &[char], col: usize) -> (String, usize) {
        let mut newindent = self.indent_cols(line);
        let (mut lead_len, part) = leader_len(line);
        let Some(p) = part.filter(|_| lead_len > 0) else {
            return (self.indent_string(newindent), 0);
        };
        let flags = COMMENTS[p].0;
        let mut repl: Option<&str> = None;
        let mut extra_space = false;
        let mut require_blank = false;
        for f in flags.chars() {
            match f {
                'b' => require_blank = true,
                's' | 'm' => {
                    let (middle, end) = if f == 's' {
                        (COMMENTS.get(p + 1), COMMENTS.get(p + 2))
                    } else {
                        (COMMENTS.get(p), COMMENTS.get(p + 1))
                    };
                    if f == 's' {
                        require_blank = middle.is_some_and(|m| m.0.contains('b'));
                    }
                    // The comment ends on this line, before the break: no
                    // leader.
                    if let Some(end) = end {
                        let e: Vec<char> = end.1.chars().collect();
                        let before = &line[lead_len.min(col)..col.max(lead_len).min(line.len())];
                        if before.windows(e.len()).any(|w| w == &e[..]) {
                            lead_len = 0;
                        }
                    }
                    if lead_len > 0 {
                        if f == 's' {
                            repl = middle.map(|m| m.1);
                        }
                        if !white(line.get(lead_len - 1)) && (col == lead_len || require_blank) {
                            extra_space = true;
                        }
                    }
                    break;
                }
                'e' => {
                    lead_len = 0;
                    break;
                }
                'f' => {
                    repl = Some("");
                    break;
                }
                _ => {}
            }
        }
        if lead_len == 0 {
            return (self.indent_string(newindent), 0);
        }
        let mut leader: Vec<char> = line[..lead_len].to_vec();
        if let Some(r) = repl {
            let off: isize = flags
                .chars()
                .filter(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse()
                .unwrap_or(0);
            // Left adjusted: the replacement where the leader's text was,
            // the rest of the old leader blanked out (tabs kept).
            let p = leader.iter().take_while(|c| white(Some(c))).count();
            let r: Vec<char> = r.chars().collect();
            // (As many of the old leader's chars as the replacement is wide.)
            let i = (leader.len() - p).min(r.len());
            leader.splice(p..p + i, r.iter().copied());
            for c in leader.iter_mut().skip(p + r.len()) {
                if !white(Some(c)) {
                    *c = ' ';
                }
            }
            newindent = self.indent_cols(&leader);
            let mut off = off;
            if (newindent as isize) + off < 0 {
                off = -(newindent as isize);
                newindent = 0;
            } else {
                newindent = (newindent as isize + off) as usize;
            }
            while off > 0
                && leader.last() == Some(&' ')
                && !leader.iter().skip(p).any(|&c| c == '\t')
            {
                leader.pop();
                off -= 1;
            }
            if white(leader.last()) {
                extra_space = false;
            }
        }
        if extra_space {
            leader.push(' ');
        }
        // A new indent goes before the leader, for the leader's own.
        if newindent > 0 {
            let w = leader.iter().take_while(|c| white(Some(c))).count();
            leader.drain(..w);
        }
        let leader: String = leader.into_iter().collect();
        let n = leader.chars().count();
        (format!("{}{leader}", self.indent_string(newindent)), n)
    }
}
