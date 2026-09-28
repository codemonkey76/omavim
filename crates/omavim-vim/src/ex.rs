//! The `:` command line: editing it (with history), line ranges, and the
//! commands the engine runs itself: `:s` and its repeats, `:d :y :j :> :<
//! :m :t :p`, `:g` and `:v`, `:normal`, `:N`, `:set`, `:noh`. Anything else
//! (`:w`, `:q`, `:e`...) goes to the app.

use super::{Beep, Mode, Op, R, Register, Vim};
use crate::search::{self, Haystack};
use crate::text::{self, first_non_blank, last_line, line_len, line_text};
use crate::{Key, Pos, TextModel};

/// A line being typed after `:`, `/` or `?`: its text and cursor, and where
/// Up/Down have got to in the history.
#[derive(Debug, Clone, Default)]
pub(super) struct LineEdit {
    pub text: Vec<char>,
    pub pos: usize,
    /// Browsing the history: the text typed before, and the entry shown.
    browse: Option<(String, usize)>,
    /// CTRL-R is waiting for a register's name.
    ctrl_r: bool,
    /// CTRL-V: the next key goes in as it is (Esc, Enter, a Ctrl key).
    literal: bool,
}

/// What a key did to a line being typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Edited {
    Typing,
    Done,
    Cancelled,
}

impl LineEdit {
    pub fn new(text: &str) -> Self {
        let text: Vec<char> = text.chars().collect();
        Self {
            pos: text.len(),
            text,
            ..Self::default()
        }
    }

    pub fn string(&self) -> String {
        self.text.iter().collect()
    }

    /// One key, with the history to browse and a register reader for
    /// CTRL-R.
    pub fn key(
        &mut self,
        key: Key,
        history: &[String],
        register: &dyn Fn(char) -> String,
    ) -> Edited {
        if std::mem::take(&mut self.literal) {
            for c in crate::key::to_register(&[key]).chars() {
                self.insert(c);
            }
            return Edited::Typing;
        }
        if std::mem::take(&mut self.ctrl_r) {
            if let Key::Char(r) = key {
                for c in register(r).chars() {
                    // A register's line breaks come in as CR, as in Vim.
                    self.insert(if c == '\n' { '\r' } else { c });
                }
            }
            return Edited::Typing;
        }
        if !matches!(key, Key::Up | Key::Down | Key::Ctrl('p' | 'n')) {
            self.browse = None;
        }
        match key {
            Key::Enter | Key::Ctrl('m' | 'j') => return Edited::Done,
            Key::Esc | Key::Ctrl('c') => return Edited::Cancelled,
            Key::Backspace | Key::Ctrl('h') => {
                if self.text.is_empty() {
                    // Backspace on an empty line leaves it, as in Vim.
                    return Edited::Cancelled;
                }
                if self.pos > 0 {
                    self.pos -= 1;
                    self.text.remove(self.pos);
                }
            }
            Key::Delete => {
                if self.pos < self.text.len() {
                    self.text.remove(self.pos);
                } else if self.pos > 0 {
                    self.pos -= 1;
                    self.text.remove(self.pos);
                }
            }
            Key::Ctrl('u') => {
                self.text.drain(..self.pos);
                self.pos = 0;
            }
            Key::Ctrl('w') => {
                // Back over blanks, then a word or a run of other chars.
                let mut i = self.pos;
                while i > 0 && self.text[i - 1] == ' ' {
                    i -= 1;
                }
                if i > 0 {
                    let word = text::is_word_char(self.text[i - 1]);
                    while i > 0
                        && self.text[i - 1] != ' '
                        && text::is_word_char(self.text[i - 1]) == word
                    {
                        i -= 1;
                        if !word {
                            break;
                        }
                    }
                }
                self.text.drain(i..self.pos);
                self.pos = i;
            }
            Key::Ctrl('r') => self.ctrl_r = true,
            Key::Ctrl('v' | 'q') => self.literal = true,
            Key::Left => self.pos = self.pos.saturating_sub(1),
            Key::Right => self.pos = (self.pos + 1).min(self.text.len()),
            Key::Home | Key::Ctrl('b') => self.pos = 0,
            Key::End | Key::Ctrl('e') => self.pos = self.text.len(),
            Key::Up | Key::Down | Key::Ctrl('p' | 'n') => {
                let older = matches!(key, Key::Up | Key::Ctrl('p'));
                // Up and Down only show entries starting with what was typed.
                let prefix_only = matches!(key, Key::Up | Key::Down);
                let (typed, at) = self
                    .browse
                    .clone()
                    .unwrap_or_else(|| (self.string(), history.len()));
                let fits = |i: usize| !prefix_only || history[i].starts_with(&typed);
                let next = if older {
                    (0..at).rev().find(|&i| fits(i))
                } else {
                    (at + 1..history.len()).find(|&i| fits(i))
                };
                match next {
                    Some(i) => {
                        *self = Self::new(&history[i]);
                        self.browse = Some((typed, i));
                    }
                    None if !older => {
                        *self = Self::new(&typed);
                    }
                    None => self.browse = Some((typed, at)),
                }
            }
            Key::Tab => self.insert('\t'),
            Key::Char(c) => self.insert(c),
            _ => {}
        }
        Edited::Typing
    }

    fn insert(&mut self, c: char) {
        self.text.insert(self.pos, c);
        self.pos += 1;
    }
}

/// Add a line to a history: once, as its newest entry.
pub(super) fn remember(history: &mut Vec<String>, line: &str) {
    if line.is_empty() {
        return;
    }
    history.retain(|h| h != line);
    history.push(line.to_string());
    if history.len() > 100 {
        history.remove(0);
    }
}

/// Where the last `:s` left its pattern, replacement and flags, for `:&`,
/// `&`, `g&` and `~` in a replacement.
#[derive(Debug, Clone, Default)]
pub(super) struct LastSub {
    pub pattern: Option<String>,
    pub string: Option<String>,
    pub flags: String,
}

/// A command line split up: the range typed (if any), the command name,
/// a `!`, and the rest.
struct Parsed<'a> {
    range: Option<(usize, usize)>,
    name: &'a str,
    bang: bool,
    arg: &'a str,
}

fn err(v: &mut Vim, msg: &str) -> R {
    v.message = Some(msg.to_string());
    Err(Beep)
}

impl Vim {
    /// `:` pressed in normal mode (`count`: `3:` means the next three
    /// lines) or visual mode (the selected lines).
    pub(super) fn start_cmdline(&mut self, prefill: &str) {
        self.cmdline = LineEdit::new(prefill);
        self.mode = Mode::CommandLine;
    }

    /// A key on the `:` command line.
    pub(super) fn cmdline_key(&mut self, t: &mut dyn TextModel, key: Key) -> R {
        let history = self.cmd_history.clone();
        let mut edit = std::mem::take(&mut self.cmdline);
        let regs = |r: char| self.get_register(r).text.clone();
        let result = edit.key(key, &history, &regs);
        self.cmdline = edit;
        match result {
            Edited::Typing => Ok(()),
            Edited::Cancelled => {
                self.mode = Mode::Normal;
                Ok(())
            }
            Edited::Done => {
                self.mode = Mode::Normal;
                let line = self.cmdline.string();
                remember(&mut self.cmd_history, &line);
                let cmd = line.trim_start_matches([':', ' ', '\t']);
                if !cmd.trim().is_empty() {
                    self.registers.insert(
                        ':',
                        Register {
                            text: cmd.to_string(),
                            linewise: false,
                            block: None,
                        },
                    );
                }
                self.begin_group();
                let result = self.ex(t, cmd);
                // (`:s///c` keeps its change open while it asks.)
                if self.confirm.is_none() {
                    self.close_group();
                }
                self.checkpcmark(t);
                result
            }
        }
    }

    /// Run a command line: commands between `|`s one after another.
    pub(super) fn ex(&mut self, t: &mut dyn TextModel, line: &str) -> R {
        let chars: Vec<char> = line.chars().collect();
        let mut start = 0;
        let mut i = 0;
        while i <= chars.len() {
            // `:g` and `:normal` take the rest of the line, `|`s and all.
            if i == start && takes_rest(&chars[start..]) {
                i = chars.len();
            }
            let bar = chars.get(i) == Some(&'|') && (i == 0 || chars[i - 1] != '\\');
            if i == chars.len() || bar {
                let cmd: String = chars[start..i].iter().collect();
                // What the app runs is its own business (`:w|q`...).
                if self.command.is_some() {
                    let rest: String = chars[start..].iter().collect();
                    let c = self.command.take().unwrap();
                    self.command = Some(format!("{c}|{rest}"));
                    return Ok(());
                }
                self.ex_one(t, &cmd)?;
                start = i + 1;
            }
            i += 1;
        }
        Ok(())
    }

    fn ex_one(&mut self, t: &mut dyn TextModel, cmd: &str) -> R {
        // The column to keep, from before the command changes anything.
        if self.want.is_none() {
            self.want = Some(self.virtcol(t));
        }
        let parsed = match self.parse_ex(t, cmd) {
            Ok(p) => p,
            Err(e) => return err(self, &e),
        };
        let Parsed {
            range,
            name,
            bang,
            arg,
        } = parsed;
        let cl = text::line_col(t, self.cursor).0;
        let (first, last) = range.unwrap_or((cl, cl));
        // (`:normal` keeps blanks at the end.)
        let raw_arg = arg.trim_start();
        let arg = arg.trim();
        match name {
            "" => {
                // `:N`, `:'a`, `:/pat/`: go to the (last) line.
                if range.is_some() {
                    self.go_line_keep(t, last);
                }
                Ok(())
            }
            "s" | "su" | "sub" | "substitute" => self.substitute(t, first, last, arg, false),
            "&" | "&&" | "~" => {
                // `:&` repeats with new flags, `:&&` with the same ones,
                // `:~` with the last search pattern.
                let keep = if name == "&&" { "&" } else { "" };
                let pattern = if name == "~" {
                    self.last_search.as_ref().map(|s| s.pattern.clone())
                } else {
                    None
                };
                self.repeat_sub(t, first, last, &format!("{keep}{arg}"), pattern)
            }
            "d" | "de" | "del" | "delete" => {
                let (reg, count) = reg_count(arg);
                let (first, last) = with_count(t, first, last, count);
                // Neovim puts the cursor on the first line before deleting.
                self.setpcmark(t);
                self.go_line_keep(t, first);
                self.adjust_skipcol(t);
                self.reg_name = reg;
                self.apply_lines(t, Op::Delete, first, last, 0)?;
                self.reg_name = None;
                // Vim's :d leaves the cursor on the line after, at its first
                // non-blank.
                let line = first.min(last_line(t));
                self.go_line(t, line);
                Ok(())
            }
            "y" | "ya" | "yank" => {
                let (reg, count) = reg_count(arg);
                let (first, last) = with_count(t, first, last, count);
                let here = self.cursor;
                self.reg_name = reg;
                let r = self.apply_lines(t, Op::Yank, first, last, 0);
                self.reg_name = None;
                self.cursor = here;
                r
            }
            "j" | "jo" | "join" => {
                let count = arg.parse::<usize>().ok();
                let (first, mut last) = match count {
                    Some(n) => (last, last + n - 1),
                    None if range.is_none() || first == last => (first, first + 1),
                    None => (first, last),
                };
                last = last.min(last_line(t));
                if last <= first {
                    return if range.is_some() { Ok(()) } else { Err(Beep) };
                }
                self.join(t, first, last - first + 1, !bang);
                self.go_line(t, first);
                Ok(())
            }
            ">" | "<" => {
                // `:>>>` shifts three times; a count after is lines.
                let extra = arg.chars().take_while(|&c| c.to_string() == name).count();
                let count = arg[extra..].trim().parse::<usize>().ok();
                let (first, last) = with_count(t, first, last, count);
                self.setpcmark(t);
                // As Neovim's op_shift: from the column aimed for on the first
                // line, each line whose indent changes leaves the cursor at
                // the indent's end (an empty line, at 0); it ends on the last.
                self.go_line_keep(t, first);
                self.adjust_skipcol(t);
                let mut col = text::line_col(t, self.cursor).1;
                for _ in 0..=extra {
                    for line in first..=last {
                        if line_len(t, line) == 0 {
                            col = 0;
                            continue;
                        }
                        let before = text::indent(t, line);
                        self.shift_line(t, line, name == ">");
                        let after = text::indent(t, line);
                        if after != before {
                            col = after.chars().count();
                        }
                    }
                }
                // (Not moved back off the line's end: an all-blank line's
                // new indent leaves it there, as in Neovim.)
                let col = col.min(line_len(t, last));
                self.cursor = text::pos(t, last, col);
                Ok(())
            }
            "m" | "mo" | "move" => {
                let dest = self.address(t, arg)?;
                self.move_lines(t, first, last, dest)
            }
            "t" | "co" | "copy" => {
                let dest = self.address(t, arg)?;
                self.copy_lines(t, first, last, dest)
            }
            "noh" | "nohl" | "nohlsearch" => {
                self.hl = false;
                Ok(())
            }
            "se" | "set" => self.set(arg),
            "p" | "pr" | "pri" | "prin" | "print" => {
                // The last line (shown), in the column aimed for.
                self.setpcmark(t);
                self.go_line_keep(t, last);
                self.message = Some(line_text(t, last.min(last_line(t))));
                Ok(())
            }
            "g" | "gl" | "glo" | "glob" | "globa" | "global" | "v" | "vg" | "vgl" | "vglo"
            | "vglob" | "vgloba" | "vglobal" => {
                self.global(t, range, bang || name.starts_with('v'), raw_arg)
            }
            "norm" | "norma" | "normal" => self.normal(t, range, raw_arg),
            _ => {
                // The app's (`w`, `q`, `e`...).
                self.command = Some(cmd.trim().to_string());
                Ok(())
            }
        }
    }

    /// The cursor to a line, in the column it aims for (Neovim's
    /// 'nostartofline').
    fn go_line_keep(&mut self, t: &dyn TextModel, line: usize) {
        let line = line.min(last_line(t));
        let want = self.want.unwrap_or_else(|| self.virtcol(t));
        let len = line_len(t, line);
        let col = self.col_at(t, line, want).min(len.saturating_sub(1));
        self.cursor = text::pos(t, line, col);
    }

    /// The cursor to a line's first non-blank.
    pub(super) fn go_line(&mut self, t: &dyn TextModel, line: usize) {
        let line = line.min(last_line(t));
        self.cursor = text::pos(
            t,
            line,
            first_non_blank(t, line).min(line_len(t, line).saturating_sub(1)),
        );
        self.want = None;
    }

    /// An address alone (`:m 0`, `:t $`); 0 is "before the first line".
    fn address(&mut self, t: &dyn TextModel, s: &str) -> R<Option<usize>> {
        let chars: Vec<char> = s.trim().chars().collect();
        let mut i = 0;
        match self.one_address(t, &chars, &mut i, text::line_col(t, self.cursor).0) {
            Ok(Some(Some(a))) if a > last_line(t) => err(self, "E16: Invalid range").map(|_| None),
            Ok(Some(a)) => Ok(a),
            Ok(None) => err(self, "E14: Invalid address").map(|_| None),
            Err(e) => err(self, &e).map(|_| None),
        }
    }

    // ── Parsing ranges ───────────────────────────────────────────────────

    /// Split a command line into its range, name, `!` and argument.
    fn parse_ex<'a>(&mut self, t: &dyn TextModel, cmd: &'a str) -> Result<Parsed<'a>, String> {
        let chars: Vec<char> = cmd.chars().collect();
        let mut i = 0;
        let mut cur = text::line_col(t, self.cursor).0;
        let mut range: Option<(usize, usize)> = None;
        let mut addrs: Vec<Option<usize>> = Vec::new();
        loop {
            while chars.get(i) == Some(&' ') {
                i += 1;
            }
            if chars.get(i) == Some(&'%') {
                i += 1;
                addrs.push(Some(0));
                addrs.push(Some(last_line(t)));
            } else {
                let a = self.one_address(t, &chars, &mut i, cur)?;
                match a {
                    Some(line) => addrs.push(Some(line.unwrap_or(0))),
                    None => addrs.push(None),
                }
            }
            match chars.get(i) {
                Some(',') => i += 1,
                Some(';') => {
                    // The next address counts from this one.
                    if let Some(Some(l)) = addrs.last() {
                        cur = *l;
                        // (The column stays, in bytes, as in Vim.)
                        let here = text::line_col(t, self.cursor);
                        let (_, b) = self.mk(t, here);
                        let (_, col) = self.unmk(t, (cur, b));
                        self.cursor =
                            text::pos(t, cur, col.min(line_len(t, cur).saturating_sub(1)));
                    }
                    i += 1;
                }
                _ => break,
            }
        }
        let given: Vec<usize> = addrs.iter().flatten().copied().collect();
        if !given.is_empty() {
            let last = *given.last().unwrap();
            let first = if given.len() > 1 {
                given[given.len() - 2]
            } else {
                last
            };
            if first > last {
                return Err("E493: Backwards range given".into());
            }
            range = Some((first, last));
        }
        while chars.get(i) == Some(&' ') {
            i += 1;
        }
        // The name: letters, or one of the punctuation commands.
        let start = i;
        let byte = |k: usize| chars[..k].iter().map(|c| c.len_utf8()).sum::<usize>();
        if let Some(&c) = chars.get(i) {
            if c.is_ascii_alphabetic() {
                // `:s` takes its pattern straight after (`:s/a/b/`), as can
                // `:d`, `:y` and a few others (`:dx` is not a command, but
                // the name ends at the first char that isn't a letter).
                while chars.get(i).is_some_and(|c| c.is_ascii_alphabetic()) {
                    i += 1;
                }
                let name: String = chars[start..i].iter().collect();
                // `:s` followed by letters is its flags (`:sg`), and
                // `:substitute` spelled out: keep just the known names.
                if name.starts_with('s')
                    && !matches!(
                        name.as_str(),
                        "s" | "su" | "sub" | "substitute" | "se" | "set"
                    )
                    && name.chars().skip(1).all(|c| "cegiInp#lr".contains(c))
                {
                    i = start + 1;
                }
            } else if "&~<>".contains(c) {
                i += 1;
                if c == '&' && chars.get(i) == Some(&'&') {
                    i += 1;
                }
            }
        }
        let name = &cmd[byte(start)..byte(i)];
        // A line past the end: the last for a bare `:N`, else an error.
        if let Some((first, last)) = range
            && last > last_line(t)
        {
            if !name.is_empty() {
                return Err("E16: Invalid range".into());
            }
            range = Some((first.min(last_line(t)), last_line(t)));
        }
        let mut bang = false;
        if chars.get(i) == Some(&'!') && !name.is_empty() {
            bang = true;
            i += 1;
        }
        Ok(Parsed {
            range,
            name,
            bang,
            arg: &cmd[byte(i)..],
        })
    }

    /// One address at `chars[*i..]`, from line `cur`: None if there's none;
    /// Some(None) for line 0 in a `:m`/`:t` sense (before the first line).
    fn one_address(
        &mut self,
        t: &dyn TextModel,
        chars: &[char],
        i: &mut usize,
        cur: usize,
    ) -> Result<Option<Option<usize>>, String> {
        let last = last_line(t) as isize;
        // Lines are 1-based here, as typed; 0 is before the first.
        let mut line: Option<isize> = None;
        match chars.get(*i) {
            Some('.') => {
                *i += 1;
                line = Some(cur as isize + 1);
            }
            Some('$') => {
                *i += 1;
                line = Some(last + 1);
            }
            Some(c) if c.is_ascii_digit() => {
                let mut n = 0isize;
                while let Some(d) = chars.get(*i).and_then(|c| c.to_digit(10)) {
                    n = n * 10 + d as isize;
                    *i += 1;
                }
                line = Some(n);
            }
            Some('\'') => {
                let m = *chars.get(*i + 1).ok_or("E20: Mark not set")?;
                *i += 2;
                let l = self.mark(t, m).map(|(l, _)| l);
                line = Some(l.ok_or("E20: Mark not set")? as isize + 1);
            }
            Some(&d @ ('/' | '?')) => {
                // The next line (after this one) matching, or the one before.
                *i += 1;
                let mut pat = String::new();
                while let Some(&c) = chars.get(*i) {
                    *i += 1;
                    if c == d {
                        break;
                    }
                    if c == '\\' && chars.get(*i) == Some(&d) {
                        pat.push(d);
                        *i += 1;
                        continue;
                    }
                    pat.push(c);
                }
                let found = self.find_line(t, &pat, cur, d == '/')?;
                line = Some(found as isize + 1);
            }
            Some('\\') if matches!(chars.get(*i + 1), Some('/' | '?' | '&')) => {
                let d = chars[*i + 1];
                *i += 2;
                let pat = if d == '&' {
                    self.last_sub.pattern.clone()
                } else {
                    self.last_search.as_ref().map(|s| s.pattern.clone())
                }
                .ok_or("E35: No previous regular expression")?;
                let found = self.find_line_with(t, &pat, cur, d != '?')?;
                line = Some(found as isize + 1);
            }
            _ => {}
        }
        // Offsets: +N, -N, and a bare + or - for one; a number with no sign
        // after an address adds too.
        loop {
            match chars.get(*i) {
                Some(&s @ ('+' | '-')) => {
                    *i += 1;
                    let mut n = 0isize;
                    let mut digits = false;
                    while let Some(d) = chars.get(*i).and_then(|c| c.to_digit(10)) {
                        n = n * 10 + d as isize;
                        *i += 1;
                        digits = true;
                    }
                    if !digits {
                        n = 1;
                    }
                    let base = line.unwrap_or(cur as isize + 1);
                    line = Some(if s == '+' { base + n } else { base - n });
                }
                Some(c) if c.is_ascii_digit() && line.is_some() => {
                    let mut n = 0isize;
                    while let Some(d) = chars.get(*i).and_then(|c| c.to_digit(10)) {
                        n = n * 10 + d as isize;
                        *i += 1;
                    }
                    line = Some(line.unwrap() + n);
                }
                _ => break,
            }
        }
        match line {
            None => Ok(None),
            Some(l) if l < 0 => Err("E16: Invalid range".into()),
            Some(0) => Ok(Some(None)),
            Some(l) => Ok(Some(Some(l as usize - 1))),
        }
    }

    /// The line after (`forward`) or before `cur` with a match for `pat`
    /// (an empty one: the last search), wrapping round. Sets the last search.
    fn find_line(
        &mut self,
        t: &dyn TextModel,
        pat: &str,
        cur: usize,
        forward: bool,
    ) -> Result<usize, String> {
        let pat = if pat.is_empty() {
            self.last_search
                .as_ref()
                .map(|s| s.pattern.clone())
                .ok_or("E35: No previous regular expression")?
        } else {
            pat.to_string()
        };
        self.set_search(pat.clone(), forward, Default::default(), false);
        self.find_line_with(t, &pat, cur, forward)
    }

    fn find_line_with(
        &self,
        t: &dyn TextModel,
        pat: &str,
        cur: usize,
        forward: bool,
    ) -> Result<usize, String> {
        let p = search::compile(pat, self.ignorecase, self.smartcase)?;
        let h = Haystack::new(t);
        let n = last_line(t) + 1;
        let order: Vec<usize> = if forward {
            (1..=n).map(|k| (cur + k) % n).collect()
        } else {
            (1..=n).map(|k| (cur + n * 2 - k) % n).collect()
        };
        order
            .into_iter()
            .find(|&l| {
                let s = t.line_to_char(l);
                p.find_at(&h, s)
                    .is_some_and(|(m, _)| m <= s + line_len(t, l))
            })
            .ok_or_else(|| format!("E486: Pattern not found: {pat}"))
    }

    // ── Moving and copying lines ─────────────────────────────────────────

    /// `:m`: lines `first..=last` to after `dest` (None: before the first).
    fn move_lines(
        &mut self,
        t: &mut dyn TextModel,
        first: usize,
        last: usize,
        dest: Option<usize>,
    ) -> R {
        if let Some(d) = dest
            && d >= first
            && d < last
        {
            return err(self, "E134: Cannot move a range of lines into itself");
        }
        if dest == Some(last)
            || (dest.is_some_and(|d| d + 1 == first))
            || (dest.is_none() && first == 0)
        {
            // Already there: only the cursor moves.
            self.go_line_keep(t, last);
            return Ok(());
        }
        let block: String = (first..=last).map(|l| line_text(t, l) + "\n").collect();
        let n = last - first + 1;
        // The marks go with the lines (Vim's do_move): set them by the move,
        // not by the insert and delete it's made of.
        let before = self.marks.clone();
        let d = dest.map_or(-1, |d| d as isize);
        let map = move |l: usize| -> usize {
            let li = l as isize;
            let (f, la) = (first as isize, last as isize);
            let out = if li >= f && li <= la {
                if d >= la {
                    li + (d - la)
                } else {
                    li - (f - (d + 1))
                }
            } else if d >= la && li > la && li <= d {
                li - n as isize
            } else if d < f && li > d && li < f {
                li + n as isize
            } else {
                li
            };
            out as usize
        };
        // Put the copy in first, then delete the original.
        let after = self.insert_lines(t, dest, &block);
        let (first, last) = if dest.is_none_or(|d| d < first) {
            (first + n, last + n)
        } else {
            (first, last)
        };
        self.delete_lines(t, first, last);
        let end = if after > first { after - n } else { after };
        self.marks = before;
        self.marks.map_lines(map);
        self.go_line_keep(t, end + n - 1);
        Ok(())
    }

    /// `:t`: a copy of lines `first..=last` after `dest`.
    fn copy_lines(
        &mut self,
        t: &mut dyn TextModel,
        first: usize,
        last: usize,
        dest: Option<usize>,
    ) -> R {
        let block: String = (first..=last).map(|l| line_text(t, l) + "\n").collect();
        let at = self.insert_lines(t, dest, &block);
        self.go_line_keep(t, at + last - first);
        Ok(())
    }

    /// Insert whole lines (`block` ends in a line break) after line `dest`
    /// (None: before the first). Returns the first new line.
    fn insert_lines(&mut self, t: &mut dyn TextModel, dest: Option<usize>, block: &str) -> usize {
        match dest {
            None => {
                self.edit(t, 0..0, block);
                0
            }
            Some(d) => {
                let at = text::pos(t, d, line_len(t, d));
                let body = block.strip_suffix('\n').unwrap_or(block);
                self.edit(t, at..at, &format!("\n{body}"));
                d + 1
            }
        }
    }

    fn delete_lines(&mut self, t: &mut dyn TextModel, first: usize, last: usize) {
        self.edit_hint = Some(super::EditHint::Lines(first, last));
        let end = text::pos(t, last, line_len(t, last));
        if last < last_line(t) {
            let (s, e) = (t.line_to_char(first), t.line_to_char(last + 1));
            self.edit(t, s..e, "");
        } else if first > 0 {
            let s = text::pos(t, first - 1, line_len(t, first - 1));
            self.edit(t, s..end, "");
        } else {
            self.edit(t, 0..end, "");
        }
    }

    // ── :set ─────────────────────────────────────────────────────────────

    /// `:set ic`, `:set noic`, `:set ic!`, `:set invic`, `:set ic?`,
    /// `:set ts=4`, `:set ts?`, `:set ic&`.
    fn set(&mut self, arg: &str) -> R {
        let mut shown = Vec::new();
        for item in arg.split_whitespace() {
            let (name, value) = match item.split_once(['=', ':']) {
                Some((n, v)) => (n, Some(v)),
                None => (item, None),
            };
            let (name, query) = match name.strip_suffix('?') {
                Some(n) => (n, true),
                None => (name, false),
            };
            let (name, reset) = match name.strip_suffix('&') {
                Some(n) => (n, true),
                None => (name, false),
            };
            let (name, toggle) = match name.strip_suffix('!') {
                Some(n) => (n, true),
                None => (name, false),
            };
            let (name, on, toggle) =
                if let Some(n) = name.strip_prefix("no").filter(|n| bool_option(n)) {
                    (n, false, toggle)
                } else if let Some(n) = name.strip_prefix("inv").filter(|n| bool_option(n)) {
                    (n, true, true)
                } else {
                    (name, true, toggle)
                };
            let full = canonical(name);
            let Some(full) = full else {
                return err(self, &format!("E518: Unknown option: {name}"));
            };
            if bool_option(full) {
                let cur = self.flag(full);
                if query {
                    shown.push(format!("{}{full}", if cur { "  " } else { "no" }));
                    continue;
                }
                let v = if reset {
                    matches!(full, "wrapscan" | "hlsearch")
                } else if toggle {
                    !cur
                } else {
                    on
                };
                self.set_flag(full, v);
                if full == "hlsearch" && v {
                    self.hl = true;
                }
            } else {
                let cur = match full {
                    "tabstop" => self.tabstop,
                    _ => self.shiftwidth,
                };
                match value {
                    None if query || !reset => shown.push(format!("  {full}={cur}")),
                    None => match full {
                        "tabstop" => self.tabstop = 8,
                        _ => self.shiftwidth = 8,
                    },
                    Some(v) => {
                        let Ok(n) = v.parse::<usize>() else {
                            return err(self, &format!("E521: Number required after =: {item}"));
                        };
                        if n == 0 && full == "tabstop" {
                            return err(self, &format!("E487: Argument must be positive: {item}"));
                        }
                        match full {
                            "tabstop" => self.tabstop = n,
                            _ => self.shiftwidth = if n == 0 { self.tabstop } else { n },
                        }
                    }
                }
            }
        }
        if !shown.is_empty() {
            self.message = Some(shown.join(" "));
        }
        Ok(())
    }

    fn flag(&self, name: &str) -> bool {
        match name {
            "ignorecase" => self.ignorecase,
            "smartcase" => self.smartcase,
            "wrapscan" => self.wrapscan,
            _ => self.hlsearch,
        }
    }

    fn set_flag(&mut self, name: &str, v: bool) {
        match name {
            "ignorecase" => self.ignorecase = v,
            "smartcase" => self.smartcase = v,
            "wrapscan" => self.wrapscan = v,
            _ => self.hlsearch = v,
        }
    }
}

/// The options `:set` knows, by their names and short names.
fn canonical(name: &str) -> Option<&'static str> {
    Some(match name {
        "ignorecase" | "ic" => "ignorecase",
        "smartcase" | "scs" => "smartcase",
        "wrapscan" | "ws" => "wrapscan",
        "hlsearch" | "hls" => "hlsearch",
        "tabstop" | "ts" => "tabstop",
        "shiftwidth" | "sw" => "shiftwidth",
        _ => return None,
    })
}

fn bool_option(name: &str) -> bool {
    canonical(name).is_some_and(|n| !matches!(n, "tabstop" | "shiftwidth"))
}

/// A `:d`/`:y` argument: a register name, then a count.
/// `:s///c` asking about each match: what it's substituting, where it's
/// got to, and what it's done.
#[derive(Debug, Clone)]
pub(super) struct Confirm {
    pattern: String,
    string: String,
    ic: bool,
    scs: bool,
    all: bool,
    /// The pattern can match a line break (it goes on across lines).
    multiline: bool,
    /// The range's first line, and its last (as lines come and go).
    first: usize,
    last: usize,
    /// The match being asked about, and where the next search starts.
    current: (Pos, Pos),
    groups: Vec<Option<(Pos, Pos)>>,
    next_from: Pos,
    subs: usize,
    /// The last line substituted on, and where that substitution ended (an
    /// empty match there isn't one).
    last_hit: Option<usize>,
    after_sub: Option<Pos>,
}

impl Confirm {
    pub(super) fn current(&self) -> std::ops::Range<Pos> {
        self.current.0..self.current.1
    }
}

impl Vim {
    fn start_confirm(
        &mut self,
        t: &mut dyn TextModel,
        first: usize,
        last: usize,
        pattern: &str,
        string: &str,
        flags: &str,
    ) -> R {
        let ic = if flags.contains('I') {
            false
        } else {
            flags.contains('i') || self.ignorecase
        };
        let scs = self.smartcase && !flags.contains('i');
        if let Err(e) = search::compile(pattern, ic, scs) {
            return err(self, &e);
        }
        let forward = self.last_search.as_ref().is_none_or(|s| s.forward);
        self.set_search(pattern.to_string(), forward, Default::default(), false);
        let mut c = Confirm {
            pattern: pattern.to_string(),
            string: string.to_string(),
            ic,
            scs,
            all: flags.contains('g'),
            multiline: pattern.contains("\\n") || pattern.contains("\\_"),
            first,
            last,
            current: (0, 0),
            groups: Vec::new(),
            next_from: t.line_to_char(first),
            subs: 0,
            last_hit: None,
            after_sub: None,
        };
        if !self.next_confirm(t, &mut c) {
            return if flags.contains('e') {
                Ok(())
            } else {
                err(self, &format!("E486: Pattern not found: {pattern}"))
            };
        }
        // Finding a match is a jump (from where the cursor was), and the
        // substitutions are one change.
        self.setpcmark(t);
        self.begin_group();
        self.ask(t, c);
        Ok(())
    }

    /// Find the next match to ask about, from `c.next_from`, in the range.
    fn next_confirm(&self, t: &dyn TextModel, c: &mut Confirm) -> bool {
        let Ok(pat) = search::compile(&c.pattern, c.ic, c.scs) else {
            return false;
        };
        let h = Haystack::with_final_newline(t);
        let last = c.last.min(last_line(t));
        let range_end = text::pos(t, last, line_len(t, last));
        let mut at = c.next_from;
        loop {
            if at > range_end {
                return false;
            }
            let Some(groups) = pat.captures_at(&h, at) else {
                return false;
            };
            let Some((s, e)) = groups[0] else {
                return false;
            };
            if s > range_end {
                return false;
            }
            // An empty match straight after a substitution isn't one.
            if s == e && Some(s) == c.after_sub {
                at = s + 1;
                continue;
            }
            c.current = (s, e);
            c.groups = groups;
            return true;
        }
    }

    /// Show the match and the question; the answer comes to confirm_key.
    fn ask(&mut self, t: &dyn TextModel, c: Confirm) {
        self.cursor = c.current.0;
        self.message = Some(format!(
            "replace with {}? (y)es/(n)o/(a)ll/(q)uit/(l)ast/scroll up(^E)/down(^Y)",
            c.string
        ));
        self.confirm = Some(c);
        self.scroll_to_cursor(t);
    }

    /// A key while `:s///c` asks.
    pub(super) fn confirm_key(&mut self, t: &mut dyn TextModel, key: Key) -> R {
        let Some(mut c) = self.confirm.take() else {
            return Ok(());
        };
        match key {
            Key::Char('y') => {
                let from = self.confirm_replace(t, &mut c);
                self.confirm_next(t, c, from);
            }
            Key::Char('l') => {
                // The last: this one, then stop.
                self.confirm_replace(t, &mut c);
                self.finish_confirm(t, c, true);
            }
            Key::Char('n') => {
                let (s, e) = c.current;
                let from = self.confirm_on(t, &c, s, e, s == e);
                self.confirm_next(t, c, from);
            }
            Key::Char('a') => {
                // This one and the rest, without asking.
                loop {
                    c.next_from = self.confirm_replace(t, &mut c);
                    if !self.next_confirm(t, &mut c) {
                        break;
                    }
                }
                self.finish_confirm(t, c, false);
            }
            Key::Char('q') | Key::Esc | Key::Ctrl('c') => self.finish_confirm(t, c, true),
            Key::Ctrl('e') | Key::Ctrl('y') => {
                // Only as far as keeps the match in view (scrollup_clamp).
                let top = self.top;
                let _ = self.scroll(t, super::Scroll::Line(key == Key::Ctrl('e')), None);
                if self.cursor != c.current.0 {
                    self.top = top;
                    self.cursor = c.current.0;
                }
                self.confirm = Some(c);
            }
            // Anything else: it asks again.
            _ => self.confirm = Some(c),
        }
        Ok(())
    }

    /// Substitute the match asked about; where to look for the next one.
    fn confirm_replace(&mut self, t: &mut dyn TextModel, c: &mut Confirm) -> Pos {
        let (s, e) = c.current;
        let h = Haystack::with_final_newline(t);
        let with = expand(&c.string, &h, &c.groups);
        let e = e.min(t.len_chars());
        let lines_before = t.len_lines();
        // Undo and redo come back to the first line changed, at its start.
        self.set_undo_cursor(t.line_to_char(t.char_to_line(s.min(t.len_chars()))));
        self.edit_hint = Some(super::EditHint::Substitute);
        self.edit(t, s..e, &with);
        c.subs += 1;
        c.last_hit = Some(t.char_to_line(s.min(t.len_chars())));
        // Lines a matched line break joined go from the range.
        c.last = (c.last + t.len_lines()).saturating_sub(lines_before);
        let after = s + with.chars().count();
        let empty = s == e;
        c.after_sub = Some(after);
        // The cursor goes to the start of the line (that the replacement
        // ends on), as Vim's do_sub.
        self.cursor = t.line_to_char(t.char_to_line(after.min(t.len_chars())));
        self.confirm_on(t, c, s, after, empty)
    }

    /// Where to look after a match at `s`, with `after` the place after it
    /// (or after what replaced it): with g, on along its line, unless that's
    /// at the line's end (Vim's "lastone"); else the next line. An empty
    /// match is found again where it was, and skipped: one past it.
    fn confirm_on(&self, t: &dyn TextModel, c: &Confirm, s: Pos, after: Pos, empty: bool) -> Pos {
        let at = after + usize::from(empty);
        let line = t.char_to_line(after.saturating_sub(1).max(s).min(t.len_chars()));
        let end = text::pos(t, line, line_len(t, line));
        if c.all && (at < end || (at == end && c.multiline)) {
            at
        } else {
            next_line_start(t, after.saturating_sub(1).max(s))
        }
    }

    fn confirm_next(&mut self, t: &mut dyn TextModel, mut c: Confirm, from: Pos) {
        c.next_from = from;
        if self.next_confirm(t, &mut c) {
            self.ask(t, c);
        } else {
            self.finish_confirm(t, c, true);
        }
    }

    /// Done asking: the marks and the cursor as Vim's do_sub leaves them
    /// (on the last match asked about, unless `a` took over).
    fn finish_confirm(&mut self, t: &mut dyn TextModel, c: Confirm, asking: bool) {
        self.message = None;
        if c.subs > 0 {
            let last = c.last.min(last_line(t));
            self.marks.op_start = Some((c.first, 0));
            self.marks.op_end = Some((last, 0));
            if !asking {
                let hit = c.last_hit.unwrap_or(last).min(last_line(t));
                self.go_line(t, hit);
            }
        }
        // (Left on the match asked about even at a line's end, as Vim.)
        if !asking {
            self.clamp(t);
        }
        self.close_group();
    }
}

/// The start of the line after the one `at` is on (past the end if none).
fn next_line_start(t: &dyn TextModel, at: Pos) -> Pos {
    let l = t.char_to_line(at.min(t.len_chars()));
    if l + 1 >= t.len_lines() {
        t.len_chars() + 1
    } else {
        t.line_to_char(l + 1)
    }
}

/// Whether a command (after its range) is one that takes the rest of the
/// line as its argument, `|`s included: `:g`, `:v`, `:normal`.
fn takes_rest(chars: &[char]) -> bool {
    let mut i = 0;
    // Skip the range, as written.
    while let Some(&c) = chars.get(i) {
        match c {
            ' ' | '\t' | '.' | '$' | '%' | ',' | ';' | '+' | '-' => i += 1,
            c if c.is_ascii_digit() => i += 1,
            '\'' | '\\' => i += 2,
            '/' | '?' => {
                i += 1;
                while let Some(&p) = chars.get(i) {
                    i += 1;
                    if p == '\\' {
                        i += 1;
                    } else if p == c {
                        break;
                    }
                }
            }
            _ => break,
        }
    }
    let name: String = chars
        .get(i..)
        .unwrap_or_default()
        .iter()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    matches!(
        name.as_str(),
        "g" | "gl"
            | "glo"
            | "glob"
            | "globa"
            | "global"
            | "v"
            | "vg"
            | "vgl"
            | "vglo"
            | "vglob"
            | "vgloba"
            | "vglobal"
            | "norm"
            | "norma"
            | "normal"
    )
}

impl Vim {
    /// `:g/pat/cmd` (`:v` and `:g!`: the lines without a match): the lines
    /// in the range (all, by default) with a match are marked, then `cmd`
    /// (`:p` if none) runs on each one still marked, from its start, as
    /// Neovim's ex_global and global_exe.
    fn global(
        &mut self,
        t: &mut dyn TextModel,
        range: Option<(usize, usize)>,
        invert: bool,
        arg: &str,
    ) -> R {
        let whole = (0, last_line(t));
        if self.global_busy && range.is_some_and(|r| r != whole) {
            return err(self, "E147: Cannot do :global recursive with a range");
        }
        let mut chars = arg.chars();
        let Some(delim) = chars.next() else {
            return err(self, "E148: Regular expression missing from global");
        };
        if delim.is_alphanumeric() || "\\\"|".contains(delim) {
            return err(
                self,
                "E146: Regular expressions can't be delimited by letters",
            );
        }
        let mut pattern = String::new();
        while let Some(c) = chars.next() {
            if c == delim {
                break;
            }
            pattern.push(c);
            if c == '\\'
                && let Some(n) = chars.next()
            {
                pattern.push(n);
            }
        }
        let cmd: String = chars.collect();
        let pattern = if pattern.is_empty() {
            match &self.last_search {
                Some(s) => s.pattern.clone(),
                None => return err(self, "E35: No previous regular expression"),
            }
        } else {
            pattern
        };
        let pat = match search::compile(&pattern, self.ignorecase, self.smartcase) {
            Ok(p) => p,
            Err(e) => return err(self, &e),
        };
        // It's the last search pattern now, and in the search history.
        let forward = self.last_search.as_ref().is_none_or(|s| s.forward);
        self.set_search(pattern.clone(), forward, Default::default(), false);
        remember(&mut self.search_history, &pattern);
        let h = Haystack::with_final_newline(t);
        let matches = |line: usize| {
            let start = t.line_to_char(line);
            let end = text::pos(t, line, line_len(t, line));
            pat.find_at(&h, start).is_some_and(|(s, _)| s <= end)
        };
        let cmd = if cmd.trim().is_empty() {
            "p".to_string()
        } else {
            cmd
        };
        if self.global_busy {
            // Nested: on the line the outer one is on.
            let line = text::line_col(t, self.cursor).0;
            if matches(line) != invert {
                self.global_one(t, &cmd, line)?;
            }
            return Ok(());
        }
        let (first, last) = range.unwrap_or(whole);
        let marked: Vec<usize> = (first..=last.min(whole.1))
            .filter(|&l| matches(l) != invert)
            .collect();
        if marked.is_empty() {
            self.message = Some(if invert {
                format!("Pattern found in every line: {pattern}")
            } else {
                format!("Pattern not found: {pattern}")
            });
            return Ok(());
        }
        self.marks.global = marked;
        self.setpcmark(t);
        self.global_busy = true;
        self.global_beginline = false;
        let mut result = Ok(());
        while let Some(i) = (0..self.marks.global.len()).min_by_key(|&i| self.marks.global[i]) {
            let line = self.marks.global.remove(i);
            if line > last_line(t) {
                continue;
            }
            result = self.global_one(t, &cmd, line);
            // An error stops it.
            if result.is_err() {
                break;
            }
        }
        self.marks.global.clear();
        self.global_busy = false;
        if std::mem::take(&mut self.global_beginline) {
            let line = text::line_col(t, self.cursor).0;
            self.go_line(t, line);
        } else {
            self.clamp(t);
        }
        self.want = None;
        result
    }

    /// (The column aimed for stays as it was, as in Vim.)
    fn global_one(&mut self, t: &mut dyn TextModel, cmd: &str, line: usize) -> R {
        self.cursor = t.line_to_char(line);
        self.ex(t, cmd)
    }

    /// `:normal {keys}`: the keys typed in normal mode (on each line of a
    /// range, from its start). A command they leave unfinished is ended as
    /// if by Esc (or CTRL-C on the command line); a key that fails drops the
    /// rest. Their changes are part of the command's undo step.
    fn normal(&mut self, t: &mut dyn TextModel, range: Option<(usize, usize)>, arg: &str) -> R {
        if arg.is_empty() {
            return err(self, "E471: Argument required");
        }
        let keys = crate::key::from_register(arg);
        let lines: Vec<Option<usize>> = match range {
            Some((first, last)) => (first..=last).map(Some).collect(),
            None => vec![None],
        };
        self.macro_depth += 1;
        self.normal_depth += 1;
        for line in lines {
            if let Some(l) = line {
                if l > last_line(t) {
                    break;
                }
                self.cursor = t.line_to_char(l);
            }
            // The keys start from the cursor's own column, and the view
            // comes to the cursor before each command (Neovim's exec_normal).
            self.want = None;
            self.scroll_to_cursor(t);
            for &k in &keys {
                if self.key(t, k).is_err() {
                    break;
                }
            }
            for _ in 0..4 {
                let key = match self.mode {
                    Mode::Insert
                    | Mode::Replace
                    | Mode::Visual
                    | Mode::VisualLine
                    | Mode::VisualBlock => Key::Esc,
                    Mode::CommandLine => Key::Ctrl('c'),
                    _ if !self.pending.is_empty() => Key::Esc,
                    _ => break,
                };
                let _ = self.key(t, key);
            }
        }
        self.macro_depth -= 1;
        self.normal_depth -= 1;
        Ok(())
    }
}

fn reg_count(arg: &str) -> (Option<char>, Option<usize>) {
    let arg = arg.trim();
    let mut chars = arg.chars();
    match chars.next() {
        Some(c) if !c.is_ascii_digit() => {
            let rest: String = chars.collect();
            (Some(c), rest.trim().parse().ok())
        }
        _ => (None, arg.parse().ok()),
    }
}

/// A count after a command: that many lines from the range's last.
fn with_count(
    t: &dyn TextModel,
    first: usize,
    last: usize,
    count: Option<usize>,
) -> (usize, usize) {
    match count {
        Some(n) if n > 0 => (last, (last + n - 1).min(last_line(t))),
        _ => (first, last),
    }
}

// ── :substitute ──────────────────────────────────────────────────────────

/// A `:s` argument, `/pattern/string/flags count`: split at its delimiter.
struct SubArgs {
    pattern: String,
    string: String,
    flags: String,
    count: Option<usize>,
}

fn parse_sub(arg: &str) -> Result<SubArgs, String> {
    let mut chars = arg.chars().peekable();
    let delim = chars.next().unwrap_or('/');
    if delim.is_alphanumeric() || "\\\"|".contains(delim) {
        return Err("E146: Regular expressions can't be delimited by letters".into());
    }
    // The pattern: up to an unescaped delimiter (`\/` stays, for the
    // pattern to read as a plain `/`).
    let mut pattern = String::new();
    let mut ended = false;
    while let Some(c) = chars.next() {
        if c == delim {
            ended = true;
            break;
        }
        pattern.push(c);
        if c == '\\'
            && let Some(n) = chars.next()
        {
            pattern.push(n);
        }
    }
    let mut string = String::new();
    let mut rest = String::new();
    if ended {
        // The replacement: `\/` is a plain delimiter char in it.
        let mut closed = false;
        while let Some(c) = chars.next() {
            if c == delim {
                closed = true;
                break;
            }
            if c == '\\' {
                match chars.next() {
                    Some(n) if n == delim => string.push(n),
                    Some(n) => {
                        string.push('\\');
                        string.push(n);
                    }
                    None => string.push('\\'),
                }
            } else {
                string.push(c);
            }
        }
        if closed {
            rest = chars.collect();
        }
    }
    let rest = rest.trim_start();
    let flags: String = rest
        .chars()
        .take_while(|c| "&cegiInp#lr".contains(*c))
        .collect();
    let tail = rest[flags.len()..].trim();
    let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !tail[digits.len()..].trim().is_empty() {
        return Err(format!(
            "E488: Trailing characters: {}",
            tail[digits.len()..].trim()
        ));
    }
    let count = digits.parse().ok();
    Ok(SubArgs {
        pattern,
        string,
        flags,
        count,
    })
}

/// A replacement for one match: `&` and `\0` the match, `\1`.. a group,
/// `\r` a line break, `\n` a NUL, `\t` a tab, and the case changes `\u \l`
/// (the next char) and `\U \L` (until `\E` or `\e`).
fn expand(string: &str, h: &Haystack, groups: &[Option<(usize, usize)>]) -> String {
    let piece = |g: usize| -> String {
        groups
            .get(g)
            .copied()
            .flatten()
            .map(|(s, e)| h.text_between(s, e))
            .unwrap_or_default()
    };
    let mut out = String::new();
    let mut one: Option<bool> = None; // \u (true) or \l for the next char
    let mut span: Option<bool> = None; // \U (true) or \L until \E
    let push = |out: &mut String, s: &str, one: &mut Option<bool>, span: Option<bool>| {
        for c in s.chars() {
            let c = match one.take().or(span) {
                Some(true) => c.to_uppercase().collect::<String>(),
                Some(false) => c.to_lowercase().collect::<String>(),
                None => c.to_string(),
            };
            out.push_str(&c);
        }
    };
    let mut chars = string.chars();
    while let Some(c) = chars.next() {
        match c {
            '&' => push(&mut out, &piece(0), &mut one, span),
            '\r' => out.push('\n'),
            '\\' => match chars.next() {
                Some(d @ '0'..='9') => {
                    push(&mut out, &piece(d as usize - '0' as usize), &mut one, span)
                }
                Some('r') => out.push('\n'),
                Some('n') => out.push('\0'),
                Some('t') => out.push('\t'),
                Some('u') => one = Some(true),
                Some('l') => one = Some(false),
                Some('U') => span = Some(true),
                Some('L') => span = Some(false),
                Some('E' | 'e') => span = None,
                Some(o) => push(&mut out, &o.to_string(), &mut one, span),
                None => out.push('\\'),
            },
            c => push(&mut out, &c.to_string(), &mut one, span),
        }
    }
    out
}

impl Vim {
    /// `:s/pattern/string/flags count`, or with no pattern and string, a
    /// repeat of the last (`:s`, `:s g`).
    pub(super) fn substitute(
        &mut self,
        t: &mut dyn TextModel,
        first: usize,
        last: usize,
        arg: &str,
        _: bool,
    ) -> R {
        let trimmed = arg.trim_start();
        // Vim's rule: a pattern follows unless the next char is a blank, a
        // digit or one of these (then it's a repeat, with flags or a count).
        if arg.is_empty()
            || arg.starts_with(|c: char| c.is_whitespace() || "0123456789cegriIp|\"".contains(c))
        {
            return self.repeat_sub(t, first, last, trimmed, None);
        }
        let a = match parse_sub(arg) {
            Ok(a) => a,
            Err(e) => return err(self, &e),
        };
        // `~` in the string is the last one used, as typed.
        let mut string = String::new();
        let mut chars = a.string.chars();
        while let Some(c) = chars.next() {
            match c {
                '~' => string.push_str(self.last_sub.string.as_deref().unwrap_or("")),
                '\\' => {
                    string.push('\\');
                    if let Some(n) = chars.next() {
                        string.push(n);
                    }
                }
                c => string.push(c),
            }
        }
        let pattern = if a.pattern.is_empty() {
            match &self.last_search {
                Some(s) => s.pattern.clone(),
                None => return err(self, "E35: No previous regular expression"),
            }
        } else {
            a.pattern.clone()
        };
        let flags = if let Some(rest) = a.flags.strip_prefix('&') {
            format!("{}{rest}", self.last_sub.flags)
        } else {
            a.flags.clone()
        };
        self.last_sub = LastSub {
            pattern: Some(pattern.clone()),
            string: Some(string.clone()),
            flags: flags.clone(),
        };
        self.do_sub(t, first, last, &pattern, &string, &flags, a.count)
    }

    /// `:&`, `:&&`, `:~`, `&`, `g&`: the last `:s` again, on these lines.
    pub(super) fn repeat_sub(
        &mut self,
        t: &mut dyn TextModel,
        first: usize,
        last: usize,
        arg: &str,
        pattern: Option<String>,
    ) -> R {
        let arg = arg.trim();
        let flags: String = arg
            .chars()
            .take_while(|c| "&cegiInp#lr".contains(*c))
            .collect();
        let count = arg[flags.len()..].trim().parse().ok();
        let flags = match flags.strip_prefix('&') {
            Some(rest) => format!("{}{rest}", self.last_sub.flags),
            None => flags,
        };
        let Some(pattern) = pattern.or_else(|| self.last_sub.pattern.clone()) else {
            return err(self, "E35: No previous regular expression");
        };
        let string = self.last_sub.string.clone().unwrap_or_default();
        self.do_sub(t, first, last, &pattern, &string, &flags, count)
    }

    #[allow(clippy::too_many_arguments)]
    fn do_sub(
        &mut self,
        t: &mut dyn TextModel,
        first: usize,
        last: usize,
        pattern: &str,
        string: &str,
        flags: &str,
        count: Option<usize>,
    ) -> R {
        let (first, last) = with_count(t, first, last, count);
        if flags.contains('c') && !flags.contains('n') {
            if self.global_busy {
                return err(self, "E1500: omavim can't ask to confirm under :g yet");
            }
            return self.start_confirm(t, first, last, pattern, string, flags);
        }
        let all = flags.contains('g');
        let count_only = flags.contains('n');
        let quiet = flags.contains('e');
        let ic = if flags.contains('I') {
            false
        } else {
            flags.contains('i') || self.ignorecase
        };
        let pat = match search::compile(pattern, ic, self.smartcase && !flags.contains('i')) {
            Ok(p) => p,
            Err(e) => return err(self, &e),
        };
        // It becomes the last search pattern too, for `n` and highlighting.
        let forward = self.last_search.as_ref().is_none_or(|s| s.forward);
        self.set_search(pattern.to_string(), forward, Default::default(), false);

        let h = Haystack::with_final_newline(t);
        // A pattern that can match a line break goes on across lines.
        let multiline = pattern.contains("\\n") || pattern.contains("\\_");
        let start = t.line_to_char(first);
        let range_end = text::pos(t, last, line_len(t, last));
        let mut out = String::new();
        let mut copied = start; // how far the old text has been copied out
        let mut subs = 0;
        let mut lines = 0;
        let mut last_line_hit = None; // the line in the new text
        let mut first_hit = None; // the first line with a match
        let mut line = first;
        let mut pos = start;
        while line <= last {
            let line_start = t.line_to_char(line).max(pos);
            let line_end = text::pos(t, line, line_len(t, line));
            let mut at = line_start;
            let mut prev_end: Option<usize> = None;
            let mut hit = false;
            let mut next_pos = None;
            while let Some(groups) = pat.captures_at(&h, at) {
                let Some((s, e)) = groups[0] else { break };
                if s > line_end {
                    break;
                }
                // An empty match straight after a match isn't one.
                if s == e && prev_end == Some(s) {
                    at = s + 1;
                    if at > line_end || (at == line_end && !multiline) {
                        break;
                    }
                    continue;
                }
                hit = true;
                subs += 1;
                if first_hit.is_none() {
                    // The first match is a jump (Vim's do_sub).
                    self.setpcmark(t);
                }
                first_hit.get_or_insert(line);
                if !count_only {
                    out.push_str(&h.text_between(copied, s));
                    out.push_str(&expand(string, &h, &groups));
                    copied = e;
                    last_line_hit = Some(first + out.matches('\n').count());
                }
                if e > line_end {
                    // Across line breaks: go on from where it ended.
                    next_pos = Some(e);
                    break;
                }
                if !all {
                    break;
                }
                prev_end = (e > s).then_some(e);
                at = if e > s { e } else { s + 1 };
                // Once a match reaches the line's end, that's the last on it.
                if at > line_end || (at == line_end && !multiline) {
                    break;
                }
            }
            if hit {
                lines += 1;
            }
            match next_pos {
                // Past the end (the last line's break): done.
                Some(e) if e > t.len_chars() => break,
                Some(e) => {
                    pos = e;
                    line = t.char_to_line(e);
                }
                None => line += 1,
            }
        }
        if subs == 0 {
            // (Under `:g`, a line without a match isn't an error.)
            return if quiet || self.global_busy {
                Ok(())
            } else {
                err(self, &format!("E486: Pattern not found: {pattern}"))
            };
        }
        if count_only {
            // It leaves the cursor at the first non-blank of its line.
            let cl = text::line_col(t, self.cursor).0;
            self.go_line(t, cl);
            self.message = Some(format!(
                "{subs} match{} on {lines} line{}",
                if subs == 1 { "" } else { "es" },
                if lines == 1 { "" } else { "s" }
            ));
            return Ok(());
        }
        let end = copied.max(range_end).min(t.len_chars());
        out.push_str(&h.text_between(copied.min(end), end));
        // The edit starts at the first line changed (the lines before are
        // as they were), and undo comes back to it, at its start.
        let from = first_hit.map_or(start, |l| t.line_to_char(l)).max(start);
        let out: String = out.chars().skip(from - start).collect();
        self.set_undo_cursor(from);
        self.edit_hint = Some(super::EditHint::Substitute);
        self.edit(t, from..end, &out);
        let hit = last_line_hit.unwrap_or(first).min(last_line(t));
        self.marks.op_start = Some((first, 0));
        self.marks.op_end = Some((hit, 0));
        if self.global_busy {
            // `:g` puts it on the first non-blank at the end.
            self.cursor = t.line_to_char(hit);
            self.global_beginline = true;
        } else {
            self.go_line(t, hit);
        }
        if lines > 2 {
            self.message = Some(format!(
                "{subs} substitution{} on {lines} lines",
                if subs == 1 { "" } else { "s" }
            ));
        }
        Ok(())
    }
}
