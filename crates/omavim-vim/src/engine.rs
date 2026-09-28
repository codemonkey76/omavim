//! The engine: keys in, edits and cursor moves out.
//!
//! Normal and visual mode commands are parsed from the keys typed so far
//! (`pending`): each key is added and the whole command parsed again, which
//! is either incomplete (wait), invalid (beep), or complete (run it). Every
//! edit goes through [`Vim::edit`], which records it for undo.

use std::collections::HashMap;

use crate::key::{self, Key};
use crate::motion::{self, Cur, Find};
use crate::search::{self, Haystack};
use crate::wrap;

#[path = "addsub.rs"]
mod addsub;
#[path = "block.rs"]
mod block;
#[path = "ex.rs"]
mod ex;
#[path = "format.rs"]
mod format;
#[path = "marks.rs"]
mod marks;
#[path = "scroll.rs"]
mod scroll;
use crate::text::{
    self, char_at, first_non_blank, indent, is_blank, last_line, line_len, line_text,
};
use crate::textobj::{self, Found, Vis};
use crate::{Indenting, SyntaxObject};
use crate::{Mode, Pos, TextModel};
use ex::{LastSub, LineEdit};
use marks::{EditHint, Marks};
use scroll::{Dir, Scroll};

/// A command couldn't be done. Vim beeps and drops the keys typed after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Beep;

type R<T = ()> = Result<T, Beep>;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Register {
    pub text: String,
    pub linewise: bool,
    /// A block (from visual block mode): its width less one (Vim's
    /// y_width). Its lines are the text's.
    pub block: Option<usize>,
}

static EMPTY: Register = Register {
    text: String::new(),
    linewise: false,
    block: None,
};

/// A register name that can be given with `"`: the ones that can be read.
fn readable(r: char) -> bool {
    r.is_ascii_alphanumeric() || "\"-_.:%+*/".contains(r)
}

/// ... and the ones that can be written.
fn writable(r: char) -> bool {
    r.is_ascii_alphanumeric() || "\"-_+*".contains(r)
}

/// Joins text appended to a register (`"Ayy`): a line break between them
/// if either is whole lines, and then it's whole lines.
fn append(old: &Register, new: &Register) -> Register {
    if old.linewise || new.linewise {
        let body = |r: &Register| {
            if r.linewise {
                r.text.strip_suffix('\n').unwrap_or(&r.text).to_string()
            } else {
                r.text.clone()
            }
        };
        let mut text = body(old);
        if !old.text.is_empty() {
            text.push('\n');
        }
        text.push_str(&body(new));
        text.push('\n');
        Register {
            text,
            linewise: true,
            block: None,
        }
    } else {
        Register {
            text: format!("{}{}", old.text, new.text),
            linewise: false,
            block: None,
        }
    }
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
    SentenceForward,
    SentenceBack,
    /// A text object: what, and `a` (true) or `i`.
    Object(Obj, bool),
    /// `/` (true) or `?`, with the pattern typed.
    Search(bool),
    /// `n`, or `N` (true: the other way).
    SearchNext(bool),
    /// `*` `#` (forward?) and `g*` `g#` (whole: false).
    Star {
        forward: bool,
        whole: bool,
    },
    /// By screen line: `gj` `gk` `g0` `g^` `gm` `g$`, by the char after g.
    Screen(char),
    /// `H`, `M`, `L`.
    ScreenLine(char),
    /// `'x` (to the line) and `` `x `` (exactly: true).
    Mark(char, bool),
    /// `]f` `[f` (forward: true) and the like: to a syntax object's start.
    SyntaxJump(SyntaxObject, bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Obj {
    Word(bool),
    Sentence,
    Paragraph,
    Block(char, char),
    Quote(char),
    Tag,
    Syntax(SyntaxObject),
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
    /// `gq`, and `gw` (which keeps the cursor where it was).
    Format,
    FormatKeep,
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
    /// `v`, `V`, CTRL-V: start (or switch to, or leave) that visual mode.
    Visual(Mode),
    Undo,
    Redo,
    Repeat,
    Cancel,
    // Visual mode only.
    /// `o`, and `O` (in block mode, the other corner on the same line).
    SwapEnds(bool),
    /// `D` and `C`: in block mode to the end of each line, else linewise.
    OperateToEnd(Op),
    /// CTRL-E, CTRL-D, CTRL-F and the other way.
    Scroll(Scroll),
    /// `zt`, `zz`, `zb` and the others: the char after z.
    Z(char),
    /// `&` (the last `:s` on this line) and `g&` (on every line).
    SubRepeat(bool),
    /// `@:`: the last command line again.
    ExRepeat,
    /// `m{char}`.
    SetMark(char),
    /// CTRL-O (false) and CTRL-I (true).
    Jump(bool),
    /// `gv`: the last visual selection again.
    Reselect,
    /// `q{reg}`: record the keys typed into a register.
    Record(char),
    /// `@{reg}`, `@@` (`'@'`) and `Q` (None: the register last recorded).
    Execute(Option<char>),
    /// CTRL-A and CTRL-X (`sub`), in visual mode with `g` (`progressive`).
    AddSub {
        sub: bool,
        progressive: bool,
    },
}

/// Where a search puts the cursor relative to its match (`/foo/e+1`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Offset {
    #[default]
    None,
    /// `/foo/+1`: lines down (linewise).
    Line(isize),
    /// `/foo/e-1`: from the match's last char (inclusive).
    End(isize),
    /// `/foo/s+2` or `/foo/b+2`: from its first char.
    Start(isize),
}

fn parse_offset(s: &str) -> Offset {
    let num = |r: &str| -> isize {
        match r {
            "" => 0,
            "+" => 1,
            "-" => -1,
            r => r.trim_start_matches('+').parse().unwrap_or(0),
        }
    };
    match s.chars().next() {
        None => Offset::None,
        Some('e') => Offset::End(num(&s[1..])),
        Some('s' | 'b') => Offset::Start(num(&s[1..])),
        Some(_) => Offset::Line(match s {
            "+" => 1,
            "-" => -1,
            s => num(s),
        }),
    }
}

/// A search line split at its closing `/` (or `?`): the pattern and the
/// offset, if one was given.
fn split_search(text: &str, delim: char) -> (&str, Option<&str>) {
    let mut chars = text.char_indices();
    let mut in_class = false;
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' => {
                chars.next();
            }
            '[' => in_class = true,
            ']' => in_class = false,
            c if c == delim && !in_class => return (&text[..i], Some(&text[i + 1..])),
            _ => {}
        }
    }
    (text, None)
}

#[derive(Debug, Clone)]
struct LastSearch {
    pattern: String,
    forward: bool,
    offset: Offset,
    /// From `*` or `#`: 'smartcase' doesn't apply.
    no_smartcase: bool,
}

#[derive(Debug, Clone)]
struct Command {
    /// Count before the operator and before the motion, multiplied.
    count: Option<usize>,
    /// The register given with `"`.
    register: Option<char>,
    action: Action,
    /// What was typed after `/` or `?`, for a search motion.
    pattern: Option<String>,
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
    /// A join or a delete of chars: undo replaces the lines it spans (as
    /// Vim saved them), whatever it looks like.
    spanned: bool,
    /// It emptied the buffer, or was made on an empty one (Vim's
    /// UH_EMPTYBUF): undo replaces the whole buffer.
    empty_buf: bool,
}

#[derive(Debug, Clone)]
struct Group {
    edits: Vec<Edit>,
    cursor_before: Pos,
    /// The named marks before the change: undo puts back the ones that were
    /// set then (as Vim's uh_namedm).
    marks_before: std::collections::HashMap<char, (usize, usize)>,
    /// The first line saved for undo when it's above the first changed (a
    /// visual CTRL-A saves the selection's lines): where undo's cursor goes.
    top: Option<usize>,
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
    /// A block insert or change: for the block's other lines at the end.
    block: Option<block::BlockInsert>,
}

#[derive(Debug, Clone)]
enum LastChange {
    Keys {
        count: Option<usize>,
        register: Option<char>,
        keys: Vec<Key>,
    },
    /// A visual operator: the same extent from the cursor, then the operator.
    Visual {
        mode: Mode,
        lines: usize,
        last_col_or_chars: usize,
        register: Option<char>,
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
    registers: HashMap<char, Register>,
    /// Recording a macro (`q{reg}`): the register, and the keys typed.
    macro_rec: Option<(char, Vec<Key>)>,
    /// Macros running (`@b` run by `@a` makes 2).
    macro_depth: usize,
    /// The register `@@` runs, and the one `Q` runs.
    last_macro: Option<char>,
    last_recorded: Option<char>,
    /// The case of the last hex number CTRL-A changed (Vim keeps it).
    hexupper: bool,
    /// `:g` is running its command, and a command it ran wants the cursor
    /// on its line's first non-blank at the end (`:s`).
    global_busy: bool,
    global_beginline: bool,
    /// Every line has been deleted (Vim's ML_EMPTY): not the same as one
    /// empty line, for undo.
    buffer_empty: bool,
    /// `:normal` is typing its keys.
    normal_depth: usize,
    /// The selection a visual change that went on in insert mode had, for
    /// `.` (recorded when insert mode ends).
    insert_extent: Option<(Mode, usize, usize)>,
    /// `.` repeating a block: its width, from the start's screen column
    /// (Vim's redo_VIsual), whatever the end line's length.
    redo_block_width: Option<usize>,
    /// The command left the cursor where Vim does, past a line's end: not
    /// to be moved back after it.
    keep_cursor: bool,
    /// `:s///c` is asking about a match.
    confirm: Option<ex::Confirm>,
    /// Where the cursor was when the command being run was typed (Vim's
    /// oap->cursor_start).
    cmd_start: Pos,
    /// The register `""` is: the one last written (Vim's y_previous).
    unnamed: Option<char>,
    /// The register given with `"` for the command being run.
    reg_name: Option<char>,
    /// The command's motion puts even a small delete in `"1` (Vim's
    /// use_reg_one: `%`, `(`, `)`, `{`, `}`, and later `/ ? n N` and marks).
    reg_one: bool,
    /// Text written to `"+` or `"*`, for the app to put on the clipboard.
    clipboard: Option<(char, String)>,
    /// A Ctrl-R in insert mode is waiting for the register's name.
    ctrl_r: bool,
    last_search: Option<LastSearch>,
    /// The pattern typed for the search command being run.
    search_input: Option<String>,
    /// Matches of the last search are shown (until `:noh`).
    hl: bool,
    /// Something to tell the user ("search hit BOTTOM", "E486: ...").
    message: Option<String>,
    /// Vim's options of the same names.
    pub ignorecase: bool,
    pub smartcase: bool,
    pub wrapscan: bool,
    pub hlsearch: bool,
    last_find: Option<(Find, char)>,
    undo: Vec<Group>,
    redo: Vec<Group>,
    group: Option<Group>,
    session: Option<Session>,
    last_change: Option<LastChange>,
    /// Keys of the change being recorded, for `.`, and its count.
    recording: Option<Vec<Key>>,
    recording_count: Option<usize>,
    recording_register: Option<char>,
    replaying: bool,
    /// Edits made so far (undo and redo included), so the app can tell
    /// when the text has changed.
    changes: u64,
    /// The `:` command line being typed.
    cmdline: LineEdit,
    /// Lines run from `:`, and searches: oldest first.
    cmd_history: Vec<String>,
    search_history: Vec<String>,
    last_sub: LastSub,
    marks: Marks,
    /// The operator's start as it runs (Vim's oap->start), for `'[`.
    op_start: Option<(usize, usize)>,
    /// What the next edit is, for moving marks (see marks.rs).
    edit_hint: Option<EditHint>,
    /// '< and '> were set as visual mode ended (not after).
    visual_marked: bool,
    /// A finished `:` command for the app to run (`w`, `q`, ...).
    command: Option<String>,
    pub shiftwidth: usize,
    pub tabstop: usize,
    /// 'filetype', and whether `:set` changed it since the app last looked.
    filetype: String,
    filetype_set: bool,
    /// The text area: cells in a row (0: unknown, no wrapping) and rows.
    width: usize,
    height: usize,
    /// The first screen row shown: a line, and a row within it (Neovim's
    /// topline and skipcol, with 'smoothscroll').
    top: (usize, usize),
    /// Columns without wrap padding, as while Neovim runs an operator.
    no_lbr: bool,
    /// Vim's 'scroll': rows CTRL-D and CTRL-U move (None: half the window).
    scroll_lines: Option<usize>,
    /// j and k (and the arrows) move by screen line, without a count or an
    /// operator: as `gj` and `gk`.
    pub display_lines: bool,
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
            registers: HashMap::new(),
            macro_rec: None,
            macro_depth: 0,
            last_macro: None,
            last_recorded: None,
            hexupper: false,
            global_busy: false,
            global_beginline: false,
            buffer_empty: false,
            normal_depth: 0,
            insert_extent: None,
            redo_block_width: None,
            keep_cursor: false,
            confirm: None,
            cmd_start: 0,
            unnamed: None,
            reg_name: None,
            reg_one: false,
            clipboard: None,
            ctrl_r: false,
            last_search: None,
            search_input: None,
            hl: false,
            message: None,
            ignorecase: false,
            smartcase: false,
            wrapscan: true,
            hlsearch: true,
            last_find: None,
            undo: Vec::new(),
            redo: Vec::new(),
            group: None,
            session: None,
            last_change: None,
            recording: None,
            recording_count: None,
            recording_register: None,
            replaying: false,
            changes: 0,
            cmdline: LineEdit::default(),
            cmd_history: Vec::new(),
            search_history: Vec::new(),
            last_sub: LastSub::default(),
            marks: Marks::default(),
            op_start: None,
            edit_hint: None,
            visual_marked: false,
            command: None,
            shiftwidth: 8,
            tabstop: 8,
            filetype: String::new(),
            filetype_set: false,
            width: 0,
            height: 0,
            top: (0, 0),
            no_lbr: false,
            scroll_lines: None,
            display_lines: false,
        }
    }

    /// A fresh engine for other text (a file opened): no undo history or
    /// cursor, but the same registers and settings, as Vim keeps them.
    pub fn for_other_text(&self) -> Self {
        Self {
            registers: self.registers.clone(),
            unnamed: self.unnamed,
            last_search: self.last_search.clone(),
            cmd_history: self.cmd_history.clone(),
            search_history: self.search_history.clone(),
            last_sub: self.last_sub.clone(),
            hl: self.hl,
            ignorecase: self.ignorecase,
            smartcase: self.smartcase,
            wrapscan: self.wrapscan,
            hlsearch: self.hlsearch,
            last_find: self.last_find,
            shiftwidth: self.shiftwidth,
            tabstop: self.tabstop,
            width: self.width,
            height: self.height,
            display_lines: self.display_lines,
            ..Self::new()
        }
    }

    /// The text area's size: cells in a row, and rows. Lines wrap at the
    /// width, as Vim's do with 'wrap' and 'linebreak'.
    pub fn set_screen(&mut self, t: &dyn TextModel, width: usize, height: usize) {
        if (width, height) != (self.width, self.height) {
            self.width = width;
            self.height = height;
            self.top.1 = self.top.1.min(self.rows(t, self.top.0).saturating_sub(1));
            self.scroll_to_cursor(t);
        }
    }

    /// The first screen row shown: a line, and a row of it.
    pub fn top(&self) -> (usize, usize) {
        self.top
    }

    /// Rows on screen (as set_screen was told).
    pub fn screen_height(&self) -> usize {
        self.height
    }

    /// Show from this row down (the cursor moves into view if need be).
    pub fn set_top(&mut self, t: &dyn TextModel, line: usize, row: usize) {
        let line = line.min(last_line(t));
        self.top = (line, row.min(self.rows(t, line).saturating_sub(1)));
    }

    /// How `line` is laid out on screen.
    pub fn layout(&self, t: &dyn TextModel, line: usize) -> wrap::Layout {
        let chars: Vec<char> = line_text(t, line).chars().collect();
        wrap::layout(&chars, self.width, self.tabstop)
    }

    /// Screen rows `line` takes.
    fn rows(&self, t: &dyn TextModel, line: usize) -> usize {
        if self.width == 0 {
            1
        } else {
            self.layout(t, line).rows.len()
        }
    }

    /// The screen column (Vim's virtual column) where a char starts,
    /// counting tabs, wide chars and the padding wrapping adds.
    fn vcol(&self, t: &dyn TextModel, line: usize, col: usize) -> usize {
        if self.width == 0 || self.no_lbr {
            return text::vcol(t, line, col, self.tabstop);
        }
        let l = self.layout(t, line);
        l.vcols[col.min(l.vcols.len() - 1)]
    }

    /// Where a motion to screen column `vcol` puts the cursor (Vim's
    /// coladvance): on a char, or in visual mode on the line's end too.
    fn col_on(&self, t: &dyn TextModel, line: usize, vcol: usize) -> usize {
        let col = self.col_at(t, line, vcol);
        let len = line_len(t, line);
        if matches!(
            self.mode,
            Mode::Visual | Mode::VisualLine | Mode::VisualBlock
        ) {
            col
        } else {
            col.min(len.saturating_sub(1))
        }
    }

    /// The char at a screen column (the line's end if it's past it).
    fn col_at(&self, t: &dyn TextModel, line: usize, vcol: usize) -> usize {
        if self.width == 0 || self.no_lbr {
            return text::col_at_vcol(t, line, vcol, self.tabstop);
        }
        let l = self.layout(t, line);
        let len = l.vcols.len() - 1;
        (0..len).find(|&i| l.vcols[i + 1] > vcol).unwrap_or(len)
    }

    pub fn mode(&self) -> Mode {
        if self.confirm.is_some() {
            return Mode::Confirm;
        }
        if self.prompt().is_some() {
            return Mode::CommandLine;
        }
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
        matches!(
            self.mode,
            Mode::Visual | Mode::VisualLine | Mode::VisualBlock
        )
        .then_some(self.anchor)
    }

    /// The command line while it's being typed, with its `:`, `/` or `?`.
    pub fn command_line(&self) -> Option<String> {
        self.command_line_cursor().map(|(line, _)| line)
    }

    /// The command line and where its cursor is (a char index in it).
    pub fn command_line_cursor(&self) -> Option<(String, usize)> {
        if self.mode == Mode::CommandLine {
            return Some((format!(":{}", self.cmdline.string()), self.cmdline.pos + 1));
        }
        let (forward, edit) = self.prompt_edit()?;
        let c = if forward { '/' } else { '?' };
        Some((format!("{c}{}", edit.string()), edit.pos + 1))
    }

    /// A search being typed: its direction and text so far.
    fn prompt(&self) -> Option<(bool, String)> {
        self.prompt_edit().map(|(f, e)| (f, e.string()))
    }

    fn prompt_edit(&self) -> Option<(bool, LineEdit)> {
        if !matches!(
            self.mode,
            Mode::Normal | Mode::Visual | Mode::VisualLine | Mode::VisualBlock
        ) {
            return None;
        }
        let i = search_start(&self.pending)?;
        Some((
            self.pending[i] == Key::Char('/'),
            self.replay_line(&self.pending[i + 1..]),
        ))
    }

    /// A search line's keys, played into a line with the search history
    /// and registers to hand.
    fn replay_line(&self, keys: &[Key]) -> LineEdit {
        let mut edit = LineEdit::default();
        let regs = |r: char| self.get_register(r).text.clone();
        for &k in keys {
            if edit.key(k, &self.search_history, &regs) != ex::Edited::Typing {
                break;
            }
        }
        edit
    }

    /// A message for the user, once: "search hit BOTTOM, continuing at
    /// TOP", "E486: Pattern not found: x", ...
    pub fn take_message(&mut self) -> Option<String> {
        self.message.take()
    }

    /// The search pattern to show matches of: the one being typed, or the
    /// last one (with 'hlsearch', until `:noh`).
    fn shown_pattern(&self) -> Option<(search::Pattern, Option<bool>)> {
        let (text, forward, no_scs) = match self.prompt() {
            Some((forward, text)) => {
                let delim = if forward { '/' } else { '?' };
                (
                    split_search(&text, delim).0.to_string(),
                    Some(forward),
                    false,
                )
            }
            None if self.hl && self.hlsearch => {
                let s = self.last_search.as_ref()?;
                (s.pattern.clone(), None, s.no_smartcase)
            }
            None => return None,
        };
        if text.is_empty() {
            return None;
        }
        let pat = search::compile(&text, self.ignorecase, self.smartcase && !no_scs).ok()?;
        Some((pat, forward))
    }

    /// Matches to highlight on `lines`: of the search being typed, or of
    /// the last search.
    pub fn search_highlights(
        &self,
        t: &dyn TextModel,
        lines: std::ops::Range<usize>,
    ) -> Vec<std::ops::Range<Pos>> {
        let Some((pat, _)) = self.shown_pattern() else {
            return Vec::new();
        };
        let last = t.len_lines().saturating_sub(1);
        let from = t.line_to_char(lines.start.min(last));
        let to = if lines.end > last {
            t.len_chars()
        } else {
            t.line_to_char(lines.end)
        };
        let h = Haystack::new(t);
        pat.all(&h, from, to)
            .into_iter()
            .map(|(s, e)| s..e)
            .collect()
    }

    /// While a search is typed: where it would go (Vim's 'incsearch'); and
    /// while `:s///c` asks, the match it's asking about.
    pub fn search_preview(&self, t: &dyn TextModel) -> Option<std::ops::Range<Pos>> {
        if let Some(c) = &self.confirm {
            return Some(c.current());
        }
        let (pat, forward) = self.shown_pattern()?;
        let h = Haystack::new(t);
        let f = search::find(t, &h, &pat, self.cursor, forward?, 1, false, true)?;
        Some(f.start..f.end)
    }

    /// A `:` command finished with Enter, for the app to run: `w`, `wq`,
    /// `q!`, ... The engine runs what it can itself (`:12`).
    pub fn take_command(&mut self) -> Option<String> {
        self.command.take()
    }

    /// Changes so far: compare before and after a key to see if it edited.
    pub fn changes(&self) -> u64 {
        self.changes
    }

    /// The unnamed register, `""`: what `p` puts.
    pub fn register(&self) -> &Register {
        self.unnamed.map_or(&EMPTY, |r| self.get_register(r))
    }

    /// A register by name (`a`, `A` is the same, `0`, `-`, `+`, ...).
    pub fn get_register(&self, name: char) -> &Register {
        match name {
            '"' => self.register(),
            _ => self
                .registers
                .get(&name.to_ascii_lowercase())
                .unwrap_or(&EMPTY),
        }
    }

    /// What's on the desktop clipboard (`'+'`) or primary selection
    /// (`'*'`): the app reads them, as the engine can't. Text ending in a
    /// line break is whole lines.
    pub fn set_clipboard(&mut self, register: char, text: &str) {
        let reg = Register {
            text: text.to_string(),
            linewise: text.ends_with('\n'),
            block: None,
        };
        self.registers.insert(register, reg);
    }

    /// Text yanked or deleted into `"+` or `"*` since the last call, for the
    /// app to put on the clipboard or primary selection.
    pub fn take_clipboard(&mut self) -> Option<(char, String)> {
        self.clipboard.take()
    }

    /// A register's name is being typed (after `"`, or Ctrl-R in insert
    /// mode): the app refreshes `"+` and `"*` from the desktop then.
    pub fn naming_register(&self) -> bool {
        self.ctrl_r || self.pending.last() == Some(&Key::Char('"'))
    }

    /// 'filetype': the app sets it from the file's name.
    pub fn set_filetype(&mut self, filetype: &str) {
        self.filetype = filetype.to_string();
    }

    /// 'filetype', when `:set filetype=` changed it since the last call.
    pub fn take_filetype(&mut self) -> Option<String> {
        std::mem::take(&mut self.filetype_set).then(|| self.filetype.clone())
    }

    /// The file's name, for `"%`.
    pub fn set_file_name(&mut self, name: Option<&str>) {
        match name {
            Some(n) => {
                self.registers.insert(
                    '%',
                    Register {
                        text: n.to_string(),
                        linewise: false,
                        block: None,
                    },
                );
            }
            None => {
                self.registers.remove(&'%');
            }
        }
    }

    /// Yanked (`delete`: deleted) text into the registers, as Vim does: the
    /// named one if given; a yank into `"0`; a delete of lines (or with a
    /// `%`-like motion) into `"1`, shifting `"1`–`"8` along; a smaller
    /// delete into `"-`. `""` is then whichever was written.
    fn store(&mut self, text: String, linewise: bool, delete: bool) {
        self.store_register(
            Register {
                text,
                linewise,
                block: None,
            },
            delete,
        );
    }

    /// A block (lines of text, and its width) into the registers.
    fn store_block(&mut self, text: String, width: usize, delete: bool) {
        self.store_register(
            Register {
                text,
                linewise: false,
                block: Some(width),
            },
            delete,
        );
    }

    fn store_register(&mut self, reg: Register, delete: bool) {
        let linewise = reg.linewise;
        let name = self.reg_name;
        if name == Some('_') {
            return;
        }
        if let Some(n) = name {
            let lower = n.to_ascii_lowercase();
            let value = if n.is_ascii_uppercase() {
                append(self.get_register(lower), &reg)
            } else {
                reg.clone()
            };
            if lower == '+' || lower == '*' {
                self.clipboard = Some((lower, value.text.clone()));
            }
            self.registers.insert(lower, value);
            self.unnamed = Some(lower);
            if !delete {
                return;
            }
        }
        if !delete {
            self.registers.insert('0', reg);
            self.unnamed = Some('0');
            return;
        }
        let small = !linewise && !reg.text.contains('\n');
        if !small || self.reg_one {
            for n in (1..9).rev() {
                let from = char::from_digit(n, 10).unwrap();
                let to = char::from_digit(n + 1, 10).unwrap();
                match self.registers.remove(&from) {
                    Some(r) => self.registers.insert(to, r),
                    None => self.registers.remove(&to),
                };
            }
            self.registers.insert('1', reg.clone());
            if !name.is_some_and(|n| n.is_ascii_uppercase()) {
                self.unnamed = Some('1');
            }
        }
        if small && name.is_none() {
            self.registers.insert('-', reg);
            self.unnamed = Some('-');
        }
    }

    /// The register a put reads: the one given with `"`, or `""`.
    fn read_register(&self) -> R<Register> {
        let reg = match self.reg_name {
            Some(n) => self.get_register(n),
            None => self.register(),
        };
        if reg.text.is_empty() {
            return Err(Beep);
        }
        Ok(reg.clone())
    }

    // ── Macros ───────────────────────────────────────────────────────────

    /// `q{reg}`: `a`–`z` (`A`–`Z` to append) or `0`–`9`.
    fn start_recording(&mut self, c: char) -> R {
        if !c.is_ascii_alphanumeric() || self.macro_depth > 0 {
            return Err(Beep);
        }
        self.macro_rec = Some((c, Vec::new()));
        Ok(())
    }

    /// `q` again: the keys typed go in the register, less that `q` (a count
    /// or register typed before it stays). `""` stays as it was.
    fn stop_recording(&mut self) {
        let Some((c, mut keys)) = self.macro_rec.take() else {
            return;
        };
        keys.pop();
        let text = key::to_register(&keys);
        let lower = c.to_ascii_lowercase();
        let reg = match self.registers.get(&lower) {
            // Appended to the last line of what's there.
            Some(r) if c.is_ascii_uppercase() && r.linewise => Register {
                text: format!("{}{text}\n", r.text.strip_suffix('\n').unwrap_or(&r.text)),
                linewise: true,
                block: None,
            },
            Some(r) if c.is_ascii_uppercase() => Register {
                text: format!("{}{text}", r.text),
                linewise: false,
                block: None,
            },
            _ => Register {
                text,
                linewise: false,
                block: None,
            },
        };
        self.registers.insert(lower, reg);
        self.last_recorded = Some(lower);
    }

    /// The register a macro is being recorded into, if one is.
    pub fn macro_register(&self) -> Option<char> {
        self.macro_rec.as_ref().map(|(c, _)| *c)
    }

    /// `@{reg}` (`@@` the last one run, `Q` the last recorded), `count`
    /// times: its text typed as keys. A command that fails stops it (and
    /// the macros that ran it), as Vim flushes what's left to type.
    fn execute(&mut self, t: &mut dyn TextModel, reg: Option<char>, count: usize) -> R {
        let c = match reg {
            None => self.last_recorded,
            Some('@') => self.last_macro,
            Some(c) => Some(c.to_ascii_lowercase()),
        }
        .ok_or_else(|| {
            self.message = Some("E748: No previously used register".into());
            Beep
        })?;
        if !(c.is_ascii_alphanumeric() || "\"-.*+".contains(c)) || self.macro_depth >= 100 {
            return Err(Beep);
        }
        let text = self.get_register(c).text.clone();
        if text.is_empty() {
            return Err(Beep);
        }
        if reg.is_some() {
            self.last_macro = Some(c);
        }
        let keys = key::from_register(&text);
        self.begin_group();
        self.macro_depth += 1;
        let mut result = Ok(());
        'run: for _ in 0..count {
            for &k in &keys {
                result = self.key(t, k);
                if result.is_err() {
                    break 'run;
                }
            }
        }
        self.macro_depth -= 1;
        result
    }

    /// The keys of a command still being typed ("d2", "g"), for showing.
    pub fn pending(&self) -> &[Key] {
        &self.pending
    }

    fn pending_operator(&self) -> bool {
        let mut i = 0;
        loop {
            match self.pending.get(i) {
                Some(Key::Char(c)) if c.is_ascii_digit() => i += 1,
                Some(Key::Char('"')) => i += 2,
                _ => break,
            }
        }
        matches!(
            self.pending.get(i),
            Some(Key::Char('d' | 'c' | 'y' | '<' | '>'))
        ) || matches!(
            (self.pending.get(i), self.pending.get(i + 1)),
            (
                Some(Key::Char('g')),
                Some(Key::Char('u' | 'U' | '~' | 'q' | 'w'))
            )
        )
    }

    /// Handle one key. On `Err(Beep)` the command failed and Vim would drop
    /// the keys typed after it.
    pub fn key(&mut self, t: &mut dyn TextModel, key: Key) -> R {
        // Ctrl-R and the register's name aren't recorded for `.`: the text
        // it inserts is (see insert_key).
        let inserting = matches!(self.mode, Mode::Insert | Mode::Replace);
        let ctrl_r = inserting && (self.ctrl_r || key == Key::Ctrl('r'));
        // A macro being recorded takes the keys typed, not the ones a macro
        // or `.` runs.
        if self.macro_depth == 0
            && !self.replaying
            && let Some((_, keys)) = self.macro_rec.as_mut()
        {
            keys.push(key);
        }
        if !self.replaying
            && !ctrl_r
            && let Some(rec) = self.recording.as_mut()
        {
            rec.push(key);
        }
        let was_visual = matches!(
            self.mode,
            Mode::Visual | Mode::VisualLine | Mode::VisualBlock
        )
        .then_some((self.mode, self.anchor, self.cursor, self.want));
        let counted = !self.pending.is_empty()
            && self
                .pending
                .iter()
                .all(|k| matches!(k, Key::Char(c) if c.is_ascii_digit()));
        let result = match self.mode {
            Mode::Insert | Mode::Replace => self.insert_key(t, key),
            _ if self.confirm.is_some() => self.confirm_key(t, key),
            Mode::CommandLine => self.cmdline_key(t, key),
            Mode::Normal if key == Key::Char(':') && (self.pending.is_empty() || counted) => {
                // `3:` is `:.,.+2`.
                let count: usize = self
                    .pending
                    .iter()
                    .filter_map(|k| match k {
                        Key::Char(c) => Some(*c),
                        _ => None,
                    })
                    .collect::<String>()
                    .parse()
                    .unwrap_or(0);
                self.pending.clear();
                let prefill = match count {
                    0 => String::new(),
                    1 => ".".to_string(),
                    n => format!(".,.+{}", n - 1),
                };
                self.start_cmdline(&prefill);
                Ok(())
            }
            Mode::Visual | Mode::VisualLine | Mode::VisualBlock
                if key == Key::Char(':') && self.pending.is_empty() =>
            {
                // An operator on the selection: the cursor goes to its start
                // (a line selection's first column).
                let lines = self.mode == Mode::VisualLine;
                let block = (self.mode == Mode::VisualBlock).then(|| self.block_area(t));
                self.mode = Mode::Normal;
                self.cursor = self.cursor.min(self.anchor);
                if let Some(a) = block {
                    // A block's top left.
                    let col = self.col_at_vcol(t, a.top, a.start_vcol);
                    self.cursor = text::pos(t, a.top, col);
                }
                if lines {
                    let (l, _) = self.lc(t);
                    self.cursor = t.line_to_char(l);
                }
                self.clamp(t);
                self.start_cmdline("'<,'>");
                Ok(())
            }
            _ => self.command_key(t, key),
        };
        if let Some((mode, anchor, cursor, want)) = was_visual
            && !matches!(
                self.mode,
                Mode::Visual | Mode::VisualLine | Mode::VisualBlock
            )
            && !std::mem::take(&mut self.visual_marked)
        {
            let at = |p: Pos| text::line_col(t, p.min(t.len_chars()));
            let want = want.unwrap_or_else(|| {
                let (l, c) = at(cursor);
                self.vcol(t, l, c)
            });
            self.marks.visual = Some((self.mk(t, at(anchor)), self.mk(t, at(cursor)), mode, want));
        }
        if result.is_err() {
            self.pending.clear();
            if self.session.is_none() {
                self.recording = None;
                self.close_group();
            }
        }
        // (Not while `:g` runs, unless for `:normal`: Neovim brings the view
        // to the cursor before each command `:normal` runs, and not for the
        // others.)
        if self.pending.is_empty()
            && !self.replaying
            && (!self.global_busy || self.normal_depth > 0)
        {
            self.scroll_to_cursor(t);
        }
        result
    }

    // ── Parsing ──────────────────────────────────────────────────────────

    fn command_key(&mut self, t: &mut dyn TextModel, key: Key) -> R {
        self.pending.push(key);
        // `q` while recording ends it (after a count or register too).
        if self.macro_rec.is_some()
            && self.macro_depth == 0
            && let Ok((_, _, [Key::Char('q')])) = take_prefix(&self.pending)
        {
            self.pending.clear();
            self.stop_recording();
            return Ok(());
        }
        let visual = matches!(
            self.mode,
            Mode::Visual | Mode::VisualLine | Mode::VisualBlock
        );
        match parse(&self.pending, visual) {
            Parse::Incomplete => Ok(()),
            Parse::Invalid => Err(Beep),
            Parse::Done(mut cmd) => {
                let keys = std::mem::take(&mut self.pending);
                if let Some(i) = search_start(&keys) {
                    let line = self.replay_line(&keys[i + 1..keys.len() - 1]).string();
                    ex::remember(&mut self.search_history, &line);
                    cmd.pattern = Some(line);
                }
                if self.display_lines && cmd.count.is_none() && self.width > 0 {
                    match cmd.action {
                        Action::Move(Motion::Down) => {
                            cmd.action = Action::Move(Motion::Screen('j'))
                        }
                        Action::Move(Motion::Up) => cmd.action = Action::Move(Motion::Screen('k')),
                        _ => {}
                    }
                }
                self.run(t, cmd, keys)
            }
        }
    }

    // ── Running commands ─────────────────────────────────────────────────

    fn run(&mut self, t: &mut dyn TextModel, cmd: Command, keys: Vec<Key>) -> R {
        self.cmd_start = self.cursor;
        let visual = matches!(
            self.mode,
            Mode::Visual | Mode::VisualLine | Mode::VisualBlock
        );
        let changes = is_change(cmd.action, visual);
        let register = cmd.register.filter(|&r| r != '"');
        let writes = match cmd.action {
            Action::Operate(op, _) => !matches!(
                op,
                Op::ShiftRight | Op::ShiftLeft | Op::Lower | Op::Upper | Op::Toggle
            ),
            _ => false,
        };
        if writes && register.is_some_and(|r| !writable(r)) {
            return Err(Beep);
        }
        if changes && !self.replaying {
            // Record for `.` without the count, which `.` can replace, and
            // the register, which `.` can move on (`"1p...`).
            let body = match take_prefix(&keys) {
                Ok((_, _, rest)) => rest.to_vec(),
                Err(_) => keys.clone(),
            };
            self.recording = Some(body);
            self.recording_count = cmd.count;
            self.recording_register = register;
        }
        self.reg_name = register;
        self.reg_one = matches!(
            cmd.action,
            Action::Operate(
                _,
                Some(
                    // Not `%`: Neovim's is matchit's, a visual selection.
                    Motion::Search(_)
                        | Motion::SearchNext(_)
                        | Motion::Star { .. }
                        | Motion::SentenceForward
                        | Motion::SentenceBack
                        | Motion::ParagraphForward
                        | Motion::ParagraphBack
                )
            )
        );
        let visual_extent = visual.then(|| self.visual_extent(t));
        let count = cmd.count;
        self.search_input = cmd.pattern.clone();
        let result = if visual {
            self.run_visual(t, cmd)
        } else {
            self.run_normal(t, cmd)
        };
        self.reg_name = None;
        self.reg_one = false;
        self.checkpcmark(t);
        self.search_input = None;
        if result.is_ok() && changes && !self.replaying && self.session.is_none() {
            self.finish_change(count, visual_extent);
        }
        // (A visual change that goes on in insert mode is repeated on the
        // same size of selection too.)
        self.insert_extent = if self.session.is_some() {
            visual_extent
        } else {
            None
        };
        if self.session.is_none() {
            self.close_group();
        }
        result
    }

    fn finish_change(&mut self, count: Option<usize>, visual: Option<(Mode, usize, usize)>) {
        if let Some(keys) = self.recording.take() {
            self.last_change = Some(match visual {
                Some((mode, lines, last)) => LastChange::Visual {
                    mode,
                    lines,
                    last_col_or_chars: last,
                    register: self.recording_register,
                    keys,
                },
                None => LastChange::Keys {
                    count: count.or(self.recording_count),
                    register: self.recording_register,
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
            Action::Visual(mode) => {
                self.anchor = self.cursor;
                self.mode = mode;
                Ok(())
            }
            Action::Undo => self.undo(t, count),
            Action::Redo => self.redo(t, count),
            Action::Repeat => self.repeat(t, cmd.count),
            Action::Cancel => Ok(()),
            Action::SwapEnds(_) | Action::OperateToEnd(_) => Err(Beep),
            Action::Scroll(s) => self.scroll(t, s, cmd.count),
            Action::Z(c) => self.z(t, c, cmd.count),
            Action::SetMark(c) => {
                if self.set_mark(t, c) {
                    Ok(())
                } else {
                    Err(Beep)
                }
            }
            Action::Jump(forward) => {
                let n = count as isize;
                match self.jump(t, if forward { n } else { -n }) {
                    Some(m) => {
                        self.cursor = self.to_mark(t, m);
                        self.want = None;
                        Ok(())
                    }
                    None => Err(Beep),
                }
            }
            Action::Reselect => {
                let Some((a, b, mode, want)) = self.marks.visual else {
                    return Err(Beep);
                };
                let (a, b) = (self.unmk(t, a), self.unmk(t, b));
                let visual = |s: &mut Self| {
                    s.mode = mode;
                    let at = |p: (usize, usize)| {
                        let l = p.0.min(last_line(t));
                        text::pos(t, l, p.1.min(line_len(t, l)))
                    };
                    s.anchor = at(a);
                    s.cursor = at(b);
                    // The column aimed for comes back too (vi_curswant).
                    s.want = Some(want);
                };
                visual(self);
                Ok(())
            }
            Action::Record(c) => self.start_recording(c),
            Action::Execute(reg) => self.execute(t, reg, count),
            Action::AddSub { sub, .. } => self.add_sub(t, sub, count),
            Action::ExRepeat => {
                let Some(line) = self.cmd_history.last().cloned() else {
                    self.message = Some("E30: No previous command line".into());
                    return Err(Beep);
                };
                for _ in 0..count {
                    self.ex(t, &line)?;
                }
                Ok(())
            }
            Action::SubRepeat(everywhere) => {
                if everywhere {
                    // `g&` is `:%s//~/&`: the last search pattern.
                    let pattern = self.last_search.as_ref().map(|s| s.pattern.clone());
                    let last = last_line(t);
                    self.repeat_sub(t, 0, last, "&", pattern)
                } else {
                    let (line, _) = self.lc(t);
                    self.repeat_sub(t, line, line, "", None)
                }
            }
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
        let want = |s: &Self| s.want.unwrap_or_else(|| s.virtcol(t));
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
                // Visual mode can go onto the end of the line (Vim's past_line).
                let visual = matches!(
                    self.mode,
                    Mode::Visual | Mode::VisualLine | Mode::VisualBlock
                );
                let max = if op || visual {
                    len
                } else {
                    len.saturating_sub(1)
                };
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
                (Cur::new(target, self.col_on(t, target, v)), Kind::Linewise)
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
                let col = self.col_at(t, c.line, n - 1);
                (
                    Cur::new(c.line, col.min(len.saturating_sub(1))),
                    Kind::Exclusive,
                )
            }
            Motion::GotoFirst | Motion::GotoLast => {
                self.setpcmark(t);
                let line = match (m, count) {
                    (_, Some(n)) => (n - 1).min(last_line(t)),
                    (Motion::GotoFirst, None) => 0,
                    _ => last_line(t),
                };
                let v = want(self);
                self.want = Some(v);
                (Cur::new(line, self.col_on(t, line, v)), Kind::Linewise)
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
                self.setpcmark(t);
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
            Motion::SentenceForward | Motion::SentenceBack => {
                self.setpcmark(t);
                let to =
                    textobj::find_sentence(t, c, n, m == Motion::SentenceForward).ok_or(Beep)?;
                self.want = None;
                (to, Kind::Exclusive)
            }
            Motion::Object(..) => return Err(Beep),
            Motion::SyntaxJump(kind, forward) => {
                let to = textobj::syntax_jump(t, c, n, forward, kind).ok_or(Beep)?;
                self.setpcmark(t);
                self.want = None;
                (to, Kind::Exclusive)
            }
            Motion::Mark(name, exact) => {
                let Some(m) = self.mark(t, name) else {
                    self.message = Some("E20: Mark not set".into());
                    return Err(Beep);
                };
                self.setpcmark(t);
                self.want = None;
                if exact {
                    return Ok((self.to_mark(t, m), Kind::Exclusive));
                }
                let l = m.0;
                (
                    Cur::new(
                        l,
                        first_non_blank(t, l).min(line_len(t, l).saturating_sub(1)),
                    ),
                    Kind::Linewise,
                )
            }
            Motion::Screen(which) => {
                // These move the cursor as they work: put it back after.
                let here = self.cursor;
                let atend = self.want == Some(usize::MAX);
                let result = match which {
                    'j' | 'k' => {
                        let dir = if which == 'j' {
                            Dir::Forward
                        } else {
                            Dir::Backward
                        };
                        let (to, ok) = self.screengo(t, dir, n);
                        if ok { Ok(to) } else { Err(Beep) }
                    }
                    '$' => self.screen_end(t, n),
                    _ => Ok(self.screen_home(t, which)),
                };
                self.cursor = here;
                let kind = match which {
                    '$' => Kind::Inclusive,
                    'j' | 'k' if atend => Kind::Inclusive,
                    _ => Kind::Exclusive,
                };
                return result.map(|to| (to, kind));
            }
            Motion::ScreenLine(which) => {
                self.setpcmark(t);
                let here = self.cursor;
                let to = self.screen_line(t, which, n, op);
                self.cursor = here;
                return Ok((to, Kind::Linewise));
            }
            Motion::Search(forward) => {
                let text = self.search_input.take().unwrap_or_default();
                let (pat, off) = split_search(&text, if forward { '/' } else { '?' });
                if pat.is_empty() {
                    self.search_from_sub();
                }
                let last = self.last_search.as_ref();
                let (pattern, offset, no_smartcase) = if pat.is_empty() {
                    // `/<CR>` is the last search again; `//e` with a new offset.
                    let Some(l) = last else {
                        self.message = Some("E35: No previous regular expression".into());
                        return Err(Beep);
                    };
                    let offset = off.map_or(l.offset, parse_offset);
                    (l.pattern.clone(), offset, l.no_smartcase)
                } else {
                    (
                        pat.to_string(),
                        off.map_or(Offset::None, parse_offset),
                        false,
                    )
                };
                self.set_search(pattern, forward, offset, no_smartcase);
                return self.do_search(t, forward, n, self.cursor);
            }
            Motion::SearchNext(reverse) => {
                self.search_from_sub();
                let Some(l) = &self.last_search else {
                    self.message = Some("E35: No previous regular expression".into());
                    return Err(Beep);
                };
                let forward = l.forward != reverse;
                self.hl = true;
                return self.do_search(t, forward, n, self.cursor);
            }
            Motion::Star { forward, whole } => {
                let Some((start, word)) = ident_at(t, c) else {
                    self.message = Some("E348: No string under cursor".into());
                    return Err(Beep);
                };
                let keyword = |ch: Option<char>| text::class(ch, false) >= 2;
                let special = if forward { "\\/.*$^~[" } else { "\\?.*$^~[" };
                let mut pattern = String::new();
                if whole && keyword(word.chars().next()) {
                    pattern.push_str("\\<");
                }
                for ch in word.chars() {
                    if special.contains(ch) {
                        pattern.push('\\');
                    }
                    pattern.push(ch);
                }
                if whole && keyword(word.chars().last()) {
                    pattern.push_str("\\>");
                }
                self.set_search(pattern, forward, Offset::None, true);
                // The search goes from the word's start (an operator still
                // starts at the cursor).
                return self.do_search(t, forward, n, text::pos(t, c.line, start));
            }
            Motion::Match if count.is_some() => {
                // {count}%: that far through the text, rounded up.
                let n = count.unwrap_or(1);
                if n > 100 {
                    return Err(Beep);
                }
                self.setpcmark(t);
                let lines = t.len_lines();
                let line = ((lines * n).div_ceil(100)).clamp(1, lines) - 1;
                let v = want(self);
                self.want = Some(v);
                (Cur::new(line, self.col_on(t, line, v)), Kind::Linewise)
            }
            Motion::Match => {
                // No bracket on the line: Neovim (its matchit plugin) stays
                // put, no error.
                let to = match motion::match_pair(t, c) {
                    Some(to) => {
                        self.setpcmark(t);
                        to
                    }
                    None => c,
                };
                self.want = None;
                (to, Kind::Inclusive)
            }
        };
        Ok((Self::at(t, to), kind))
    }

    /// With no search pattern yet, Vim searches with the last `:s` one.
    fn search_from_sub(&mut self) {
        if self.last_search.is_none()
            && let Some(p) = self.last_sub.pattern.clone()
        {
            self.set_search(p, true, Offset::None, false);
        }
    }

    /// What the last `:s` was: its pattern, replacement and flags (for
    /// `:&`, `&`, `~`, and searching when there's no search pattern yet).
    pub fn set_last_substitute(&mut self, pattern: &str, string: &str, flags: &str) {
        self.last_sub = LastSub {
            pattern: Some(pattern.to_string()),
            string: Some(string.to_string()),
            flags: flags.to_string(),
        };
    }

    fn set_search(&mut self, pattern: String, forward: bool, offset: Offset, no_smartcase: bool) {
        self.registers.insert(
            '/',
            Register {
                text: pattern.clone(),
                linewise: false,
                block: None,
            },
        );
        self.last_search = Some(LastSearch {
            pattern,
            forward,
            offset,
            no_smartcase,
        });
        self.hl = true;
    }

    /// The last search's `count`th match from the cursor, with its offset
    /// applied: where the cursor goes and the motion's kind.
    fn do_search(
        &mut self,
        t: &dyn TextModel,
        forward: bool,
        count: usize,
        from: Pos,
    ) -> R<(Pos, Kind)> {
        let Some(s) = self.last_search.clone() else {
            return Err(Beep);
        };
        let pat = match search::compile(
            &s.pattern,
            self.ignorecase,
            self.smartcase && !s.no_smartcase,
        ) {
            Ok(p) => p,
            Err(e) => {
                self.message = Some(e);
                return Err(Beep);
            }
        };
        let h = Haystack::new(t);
        // With a char offset, start from where the match would be, so `n`
        // doesn't find the same one again.
        let mut from = {
            let (l, c) = text::line_col(t, from);
            Cur::new(l, c)
        };
        if let Offset::End(off) | Offset::Start(off) = s.offset {
            for _ in 0..off.unsigned_abs() {
                let r = if off > 0 {
                    textobj::decl(t, &mut from)
                } else {
                    textobj::incl(t, &mut from)
                };
                if r == -1 {
                    break;
                }
            }
        }
        let at_end = matches!(s.offset, Offset::End(_));
        let Some(found) = search::find(
            t,
            &h,
            &pat,
            Self::at(t, from),
            forward,
            count,
            at_end,
            self.wrapscan,
        ) else {
            self.message = Some(format!("E486: Pattern not found: {}", s.pattern));
            return Err(Beep);
        };
        self.setpcmark(t);
        self.message = found.wrapped.then(|| {
            if forward {
                "search hit BOTTOM, continuing at TOP".to_string()
            } else {
                "search hit TOP, continuing at BOTTOM".to_string()
            }
        });
        self.want = None;
        let cur_of = |p: Pos| {
            let (l, c) = text::line_col(t, p);
            Cur::new(l, c)
        };
        let step = |c: &mut Cur, n: isize| {
            for _ in 0..n.unsigned_abs() {
                let r = if n > 0 {
                    textobj::incl(t, c)
                } else {
                    textobj::decl(t, c)
                };
                if r == -1 {
                    break;
                }
            }
        };
        // A match on a line's end: the cursor goes on its last char (and
        // the motion stays exclusive, as Vim's does).
        let back = |p: Pos| {
            let (l, c) = text::line_col(t, p);
            if c > 0 && c == line_len(t, l) {
                p - 1
            } else {
                p
            }
        };
        Ok(match s.offset {
            Offset::None => (back(found.start), Kind::Exclusive),
            Offset::Start(n) => {
                let mut c = cur_of(found.start);
                step(&mut c, n);
                (Self::at(t, c), Kind::Exclusive)
            }
            Offset::End(n) => {
                let mut c = cur_of(if found.end > found.start {
                    found.end - 1
                } else {
                    found.start
                });
                step(&mut c, n);
                (Self::at(t, c), Kind::Inclusive)
            }
            Offset::Line(n) => {
                let line = t.char_to_line(found.start) as isize + n;
                let line = line.clamp(0, last_line(t) as isize) as usize;
                (t.line_to_char(line), Kind::Linewise)
            }
        })
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
        // The column aimed for before the motion (Vim updates it only at the
        // start of a command): a linewise delete goes back to it.
        let want_before = self.want;
        let n = count.unwrap_or(1);
        if let Motion::Object(obj, around) = m {
            return self.operate_object(t, op, obj, around, n);
        }
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
        let motion_end_line = el;

        // An exclusive motion ending in column 0 of a later line ends at the end
        // of the line before, or becomes linewise (Vim's `:help exclusive`).
        let mut adjusted = false;
        if kind == Kind::Exclusive && ec == 0 && el > sl {
            el -= 1;
            let start_c = text::line_col(t, start).1;
            if start_c <= first_non_blank(t, sl) {
                kind = Kind::Linewise;
                adjusted = true;
            } else {
                let len = line_len(t, el);
                end = text::pos(t, el, len);
                if len > 0 {
                    end -= 1;
                    kind = Kind::Inclusive;
                }
            }
        }
        // `gq` works on the lines the motion covers (and knows if its end
        // was moved back a line, as Vim's end_adjusted).
        if matches!(op, Op::Format | Op::FormatKeep) {
            let adjusted = el < motion_end_line;
            self.format(t, sl, el - sl + 1, op == Op::FormatKeep, adjusted);
            return Ok(());
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
        self.op_start = Some(text::line_col(t, start));
        let line_offset = matches!(m, Motion::Search(_) | Motion::SearchNext(_))
            && self
                .last_search
                .as_ref()
                .is_some_and(|s| matches!(s.offset, Offset::Line(_)));
        if kind == Kind::Linewise && line_offset {
            // A line offset (`/foo/+1`) goes to column 0, and a yank leaves
            // the cursor at the start, as Vim's does.
            self.cursor = self.cursor.min(to);
            return self.apply_lines(t, op, sl, el, 0);
        }
        if adjusted && op == Op::Delete {
            // Made linewise by the rule above: Vim goes to the first
            // non-blank, not back to the column.
            self.apply_lines(t, op, sl, el, 0)?;
            let (line, _) = self.lc(t);
            self.cursor = text::pos(t, line, first_non_blank(t, line));
            self.clamp(t);
            return Ok(());
        }
        if kind == Kind::Linewise && matches!(op, Op::Yank | Op::Lower | Op::Upper | Op::Toggle) {
            // These leave the cursor at the operator's start.
            self.cursor = self.cursor.min(to);
        }
        if kind == Kind::Linewise {
            let want = if op == Op::Delete {
                want_before
            } else {
                self.want
            };
            let v = match want {
                // After `$` a linewise delete keeps the cursor's own column.
                Some(w) if w != usize::MAX || op != Op::Delete => w,
                _ => self.vcol(t, start_cur.line, start_cur.col),
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

    /// The text object `obj` at the cursor (`around`: `a`, else `i`), or
    /// from the visual selection `vis`.
    fn object(
        &mut self,
        t: &dyn TextModel,
        obj: Obj,
        around: bool,
        n: usize,
        vis: Option<Vis>,
    ) -> Found {
        let c = self.cur(t);
        // Jumps, as Vim's text objects make them: a block or tag from the
        // cursor, and every sentence search.
        let fresh = vis.is_none_or(|v| v.anchor == c);
        if fresh && matches!(obj, Obj::Block(..) | Obj::Tag) {
            self.setpcmark(t);
        }
        textobj::take_sentence_jumps();
        let found = self.object_inner(t, obj, around, n, vis);
        let here = self.cursor;
        for j in textobj::take_sentence_jumps() {
            self.cursor = Self::at(t, j);
            self.setpcmark(t);
        }
        self.cursor = here;
        found
    }

    fn object_inner(
        &self,
        t: &dyn TextModel,
        obj: Obj,
        around: bool,
        n: usize,
        vis: Option<Vis>,
    ) -> Found {
        let c = self.cur(t);
        match obj {
            Obj::Word(big) => textobj::word(t, c, vis, n, around, big),
            Obj::Sentence => textobj::sentence(t, c, vis, n, around),
            Obj::Paragraph => textobj::paragraph(t, c, vis, n, around),
            Obj::Block(open, close) => textobj::block(t, c, vis, n, around, open, close),
            Obj::Quote(q) => textobj::quote(t, c, n, around, q).ok_or(c.into()),
            Obj::Tag => textobj::tag(t, c, vis, n, around),
            Obj::Syntax(kind) => textobj::syntax(t, c, vis, n, around, kind),
        }
    }

    fn operate_object(
        &mut self,
        t: &mut dyn TextModel,
        op: Op,
        obj: Obj,
        around: bool,
        n: usize,
    ) -> R {
        let o = match self.object(t, obj, around, n, None) {
            Ok(o) => o,
            Err(stop) => {
                self.cursor = Self::at(t, stop.cursor);
                self.clamp(t);
                return Err(Beep);
            }
        };
        let (line, col) = self.lc(t);
        let keep = self
            .want
            .filter(|&w| w != usize::MAX)
            .unwrap_or_else(|| self.vcol(t, line, col));
        self.want = None;
        let (start, mut end) = (o.start, o.end);
        let mut inclusive = o.inclusive;
        let mut linewise = o.linewise;
        let in_indent = start.col <= first_non_blank(t, start.line);
        // Vim's `:help exclusive` rule, as for motions.
        let mut adjusted = false;
        if !linewise && !inclusive && end.col == 0 && end.line > start.line {
            adjusted = true;
            end.line -= 1;
            if in_indent {
                linewise = true;
            } else {
                let len = line_len(t, end.line);
                end.col = len.saturating_sub(1);
                inclusive = len > 0;
            }
        }
        // `d` over whole lines, with only blanks around, is linewise.
        if op == Op::Delete && !linewise && end.line > start.line && in_indent {
            let after = end.col + usize::from(inclusive);
            if line_text(t, end.line).chars().skip(after).all(is_blank) {
                linewise = true;
            }
        }
        if linewise {
            if op == Op::Delete {
                self.apply_lines(t, op, start.line, end.line, keep)?;
                if adjusted {
                    let (line, _) = self.lc(t);
                    self.cursor = text::pos(t, line, first_non_blank(t, line));
                    self.clamp(t);
                }
                return Ok(());
            }
            // The cursor goes to the object's start.
            self.cursor = Self::at(t, start);
            let v = self.vcol(t, start.line, start.col);
            return self.apply_lines(t, op, start.line, end.line, v);
        }
        let from = Self::at(t, start);
        let mut to = Self::at(t, end);
        // Inclusive at a line's end takes nothing, unless it joins lines.
        if inclusive && (end.col < line_len(t, end.line) || end.line > start.line) {
            to += 1;
        }
        let to = to.max(from).min(t.len_chars());
        if to == from && inclusive && op == Op::Change {
            // An object on a line's end (`iw` on an empty line) isn't empty
            // to Vim: a change deletes nothing, into "- (an empty string).
            // (A delete on an empty line stops before that.)
            self.store(String::new(), false, true);
        }
        if to == from && op != Op::Change {
            // Nothing inside (as `di(` on `()`): just go there.
            self.cursor = from;
            self.clamp(t);
            return Ok(());
        }
        self.apply_chars(t, op, from..to)
    }

    fn operate_lines(&mut self, t: &mut dyn TextModel, op: Op, count: usize) -> R {
        let (line, col) = self.lc(t);
        if count > 1 && line == last_line(t) {
            return Err(Beep);
        }
        let count = count.min(last_line(t) - line + 1);
        self.op_start = Some((line, col));
        let v = match op {
            // Vim moves to the first non-blank before these (nv_lineop).
            Op::Change | Op::Lower | Op::Upper | Op::Toggle => {
                self.vcol(t, line, first_non_blank(t, line))
            }
            _ => self.want.unwrap_or_else(|| self.vcol(t, line, col)),
        };
        if matches!(op, Op::Lower | Op::Upper | Op::Toggle) {
            self.cursor = text::pos(t, line, first_non_blank(t, line));
        }
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
        // Neovim turns 'linebreak' off while an operator runs.
        let saved = std::mem::replace(&mut self.no_lbr, true);
        // '[ and '] (a change's are set when its insert ends), as bytes into
        // the lines as they were.
        let start = self.op_start.take().unwrap_or((first, 0));
        let start = self.mk(t, start);
        let result = self.apply_lines_inner(t, op, first, last, vcol);
        self.no_lbr = saved;
        // After any operator the column aimed for is the cursor's again.
        self.want = None;
        let last_col =
            |t: &dyn TextModel, l: usize| line_len(t, l.min(last_line(t))).saturating_sub(1);
        match op {
            Op::Yank => {
                self.marks.op_start = Some((first, 0));
                self.marks.op_end = Some((last, usize::MAX));
            }
            Op::Lower | Op::Upper | Op::Toggle => {
                self.marks.op_start = Some((first, 0));
                self.marks.op_end = Some(self.mk(t, (last, last_col(t, last))));
            }
            Op::Delete => {
                self.marks.op_start = Some(start);
                self.marks.op_end = Some(start);
            }
            Op::ShiftRight | Op::ShiftLeft => {
                self.marks.op_start = Some(start);
                self.marks.op_end = Some(self.mk(t, (last, last_col(t, last))));
                self.marks.last_change = Some((first, 0));
            }
            // (These set their own.)
            Op::Change | Op::Format | Op::FormatKeep => {}
        }
        result
    }

    fn apply_lines_inner(
        &mut self,
        t: &mut dyn TextModel,
        op: Op,
        first: usize,
        last: usize,
        vcol: usize,
    ) -> R {
        if matches!(op, Op::Format | Op::FormatKeep) {
            self.format(t, first, last - first + 1, op == Op::FormatKeep, false);
            return Ok(());
        }
        let start = t.line_to_char(first);
        let end = text::pos(t, last, line_len(t, last));
        let mut yanked = t.slice(start..end);
        yanked.push('\n');
        let place = |s: &mut Self, t: &dyn TextModel, line: usize| {
            let col = s.col_at(t, line, vcol);
            s.cursor = text::pos(t, line, col.min(line_len(t, line).saturating_sub(1)));
        };
        match op {
            Op::Format | Op::FormatKeep => unreachable!(),
            Op::Yank => {
                self.store(yanked, true, false);
                let (line, _) = self.lc(t);
                if first < line {
                    place(self, t, first);
                }
                self.clamp(t);
                Ok(())
            }
            Op::Delete => {
                self.store(yanked, true, true);
                self.begin_group();
                if last == last_line(t) && first > 0 {
                    // The last lines go with the line break before them.
                    let from = text::pos(t, first - 1, line_len(t, first - 1));
                    self.edit_hint = Some(EditHint::Lines(first, last));
                    self.edit(t, from..end, "");
                    if let Some(e) = self.group.as_mut().and_then(|g| g.edits.last_mut()) {
                        e.line = first;
                    }
                    place(self, t, first - 1);
                } else if last == last_line(t) {
                    self.edit_hint = Some(EditHint::Lines(first, last));
                    self.edit(t, 0..end, "");
                    self.cursor = 0;
                } else {
                    let to = t.line_to_char(last + 1);
                    self.edit_hint = Some(EditHint::Lines(first, last));
                    self.edit(t, start..to, "");
                    place(self, t, first);
                }
                Ok(())
            }
            Op::Change => {
                self.store(yanked, true, true);
                self.begin_group();
                let ind = indent(t, first);
                let from = t.line_to_char(first);
                self.edit_hint = Some(EditHint::Change);
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
                // The cursor stays where the caller put it: the operator's
                // start.
                let (line, _) = self.lc(t);
                if line != first {
                    place(self, t, first);
                }
                self.clamp(t);
                Ok(())
            }
        }
    }

    /// An operator over the chars in `range`.
    fn apply_chars(&mut self, t: &mut dyn TextModel, op: Op, range: std::ops::Range<Pos>) -> R {
        let saved = std::mem::replace(&mut self.no_lbr, true);
        let at = |p: Pos| self.mk(t, text::line_col(t, p.min(t.len_chars())));
        let (start, end) = (
            at(range.start),
            at(range.end.saturating_sub(1).max(range.start)),
        );
        // (Deleting nothing sets no marks: Vim's op_delete stops first.)
        let empty = range.is_empty();
        let result = self.apply_chars_inner(t, op, range);
        self.no_lbr = saved;
        self.want = None;
        match op {
            Op::Delete if empty => {}
            Op::Yank | Op::Lower | Op::Upper | Op::Toggle => {
                self.marks.op_start = Some(start);
                self.marks.op_end = Some(end);
            }
            Op::Delete => {
                self.marks.op_start = Some(start);
                self.marks.op_end = Some(start);
            }
            _ => {}
        }
        result
    }

    fn apply_chars_inner(
        &mut self,
        t: &mut dyn TextModel,
        op: Op,
        range: std::ops::Range<Pos>,
    ) -> R {
        let yanked = t.slice(range.clone());
        match op {
            Op::Format | Op::FormatKeep => {
                // On the lines the chars are on.
                let first = t.char_to_line(range.start.min(t.len_chars()));
                let last = t.char_to_line(
                    range
                        .end
                        .saturating_sub(1)
                        .max(range.start)
                        .min(t.len_chars()),
                );
                self.format(t, first, last - first + 1, op == Op::FormatKeep, false);
                Ok(())
            }
            Op::Yank => {
                self.store(yanked, false, false);
                self.cursor = range.start;
                self.clamp(t);
                Ok(())
            }
            Op::Delete => {
                if range.is_empty() {
                    // Vim still saves the line for undo (u_save_cursor): an
                    // undo step that changes nothing.
                    self.want = None;
                    self.noop_undo_step(t, range.start);
                    return Ok(());
                }
                // Undo comes back to where the deleted text began.
                self.cursor = range.start;
                self.store(yanked, false, true);
                self.begin_group();
                self.edit_hint = Some(EditHint::Spanned);
                self.edit(t, range.clone(), "");
                self.cursor = range.start;
                self.clamp(t);
                self.want = None;
                Ok(())
            }
            Op::Change => {
                if !range.is_empty() {
                    self.store(yanked, false, true);
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
                // The column from before the operator, when 'linebreak' was on.
                let (l, c) = self.lc(t);
                let saved = std::mem::replace(&mut self.no_lbr, false);
                let v = self.want.unwrap_or_else(|| self.vcol(t, l, c));
                self.no_lbr = saved;
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
        let first_len = line_len(t, line);
        for _ in 1..n {
            join_col = self.join_next(t, line, spaces);
        }
        // As Vim's do_join: '[ at the first line's end, '] at the joined
        // line's, '. on the line after (where the lines were deleted).
        self.marks.op_start = Some(self.mk(t, (line, first_len)));
        self.marks.op_end = Some(self.mk(t, (line, line_len(t, line))));
        self.marks.last_change = Some((line + 1, 0));
        self.cursor = text::pos(t, line, join_col);
        self.clamp(t);
        self.want = None;
    }

    /// Join the line after `line` onto it (with `spaces`, as `J`: its
    /// leading blanks go, and a space goes between, unless...): where it
    /// joined.
    fn join_next(&mut self, t: &mut dyn TextModel, line: usize, spaces: bool) -> usize {
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
        self.edit_hint = Some(EditHint::Join);
        self.edit(t, from..to, if space { " " } else { "" });
        cur_len
    }

    fn put(&mut self, t: &mut dyn TextModel, before: bool, count: usize) -> R {
        let reg = self.read_register()?;
        self.put_register(t, &reg, before, count)
    }

    /// Put `reg` at the cursor (before it, or after), `count` times.
    fn put_register(
        &mut self,
        t: &mut dyn TextModel,
        reg: &Register,
        before: bool,
        count: usize,
    ) -> R {
        if let Some(width) = reg.block {
            return self.put_block(t, &reg.text, width, before, count);
        }
        self.begin_group();
        let (line, col) = self.lc(t);
        // '[ where the text starts, '] on the last char of its last line.
        let (start, block_lines, last_line_len);
        if reg.linewise {
            let body = reg.text.strip_suffix('\n').unwrap_or(&reg.text);
            let block = vec![body; count].join("\n");
            let target = if before {
                let at = t.line_to_char(line);
                self.edit_hint = Some(EditHint::Above);
                self.edit(t, at..at, &format!("{block}\n"));
                line
            } else {
                let at = text::pos(t, line, line_len(t, line));
                self.edit(t, at..at, &format!("\n{block}"));
                line + 1
            };
            start = t.line_to_char(target);
            block_lines = block.matches('\n').count();
            last_line_len = block.rsplit('\n').next().map_or(0, |l| l.chars().count());
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
            start = at;
            block_lines = block.matches('\n').count();
            last_line_len = block.rsplit('\n').next().map_or(0, |l| l.chars().count());
            if block.contains('\n') {
                self.cursor = at;
                self.clamp(t);
            } else {
                self.cursor = at + block.chars().count() - 1;
            }
        }
        let (l1, c1) = text::line_col(t, start);
        let l2 = l1 + block_lines;
        let end = if !reg.linewise && block_lines == 0 {
            (l1, c1 + last_line_len.saturating_sub(1))
        } else {
            (l2, last_line_len.saturating_sub(1))
        };
        self.marks.op_start = Some(self.mk(t, (l1, c1)));
        self.marks.op_end = Some(self.mk(t, end));
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
                let mut ind = indent(t, line);
                if let Some(smart) = t.smart_indent() {
                    // Below a line that opens a block, or above one that
                    // closes it: inside it.
                    let deeper = if kind == Insert::Below {
                        opens_block(t, line, len, &smart)
                    } else {
                        closes_block(t, line)
                    };
                    if deeper {
                        ind.push_str(&smart.unit);
                    }
                }
                if kind == Insert::Below {
                    let at = text::pos(t, line, len);
                    self.edit(t, at..at, &format!("\n{ind}"));
                    self.cursor = text::pos(t, line + 1, ind.chars().count());
                } else {
                    let at = t.line_to_char(line);
                    self.edit_hint = Some(EditHint::Above);
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
        if std::mem::take(&mut self.ctrl_r) {
            // Ctrl-R x: the register's text, as if typed (and recorded as
            // typed, so `.` repeats the text, not the register).
            let Key::Char(r) = key else {
                return Ok(());
            };
            if !readable(r) {
                return Ok(());
            }
            let text = self.get_register(r).text.clone();
            for c in text.chars() {
                let k = match c {
                    '\n' => Key::Enter,
                    '\t' => Key::Tab,
                    c => Key::Char(c),
                };
                if !self.replaying
                    && let Some(rec) = self.recording.as_mut()
                {
                    rec.push(k);
                }
                self.insert_key(t, k)?;
            }
            return Ok(());
        }
        if key == Key::Ctrl('r') {
            self.ctrl_r = true;
            return Ok(());
        }
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
                if !replace && matches!(c, '}' | ']' | ')') {
                    self.outdent_closer(t);
                }
                Ok(())
            }
            Key::Tab => {
                self.type_char(t, '\t', replace);
                Ok(())
            }
            Key::Enter => {
                let (line, col) = self.lc(t);
                let mut ind: String = indent(t, line).chars().take(col).collect();
                // Between brackets just opened and their close: the close
                // goes on a line of its own, back out.
                let mut close = None;
                if let Some(smart) = t.smart_indent().filter(|_| !replace)
                    && opens_block(t, line, col, &smart)
                {
                    if let Some(ch) = char_at(t, line, col)
                        && matches!(ch, '}' | ']' | ')')
                    {
                        close = Some(ind.clone());
                    }
                    ind.push_str(&smart.unit);
                }
                self.remove_lone_autoindent(t);
                let at = self.cursor;
                // With autoindent, blanks that would start the new line go.
                let (l, c) = self.lc(t);
                let rest = line_text(t, l)
                    .chars()
                    .skip(c)
                    .take_while(|&ch| is_blank(ch))
                    .count();
                let tail = close.map(|c| format!("\n{c}")).unwrap_or_default();
                self.edit(t, at..at + rest, &format!("\n{ind}{tail}"));
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

    /// A closing bracket typed first on its line: indent it as the line
    /// its open bracket is on.
    fn outdent_closer(&mut self, t: &mut dyn TextModel) {
        if t.smart_indent().is_none() {
            return;
        }
        let (line, col) = self.lc(t);
        let Some(at) = col.checked_sub(1) else { return };
        if first_non_blank(t, line) != at {
            return;
        }
        let Some(open) = motion::match_pair(t, Cur::new(line, at)) else {
            return;
        };
        let want = indent(t, open.line);
        let start = t.line_to_char(line);
        if indent(t, line) != want {
            self.edit(t, start..start + at, &want);
            self.cursor = start + want.chars().count() + 1;
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
        let block = session.as_ref().and_then(|s| s.block.clone());
        if let Some(s) = &session {
            // `".`: what was typed, as Vim keeps it.
            let text: String = s
                .typed
                .iter()
                .flat_map(|k| match *k {
                    Key::Char(c) => vec![c],
                    Key::Enter => vec!['\n'],
                    Key::Tab => vec!['\t'],
                    Key::Backspace => vec!['\u{80}', 'k', 'b'],
                    Key::Ctrl(c) => char::from_u32(c.to_ascii_uppercase() as u32 ^ 0x40)
                        .into_iter()
                        .collect(),
                    _ => vec![],
                })
                .collect();
            self.registers.insert(
                '.',
                Register {
                    text,
                    linewise: false,
                    block: None,
                },
            );
        }
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
        // '^ where insert mode was left; '[ and '] the text inserted.
        let here = self.mk(t, text::line_col(t, self.cursor.min(t.len_chars())));
        self.marks.last_insert = Some(here);
        if let Some(s) = &self.session {
            self.marks.op_start = Some(self.mk(t, text::line_col(t, s.start.min(t.len_chars()))));
            self.marks.op_end = Some(here);
        }
        self.session = None;
        self.mode = Mode::Normal;
        let (_, col) = self.lc(t);
        if col > 0 {
            self.cursor -= 1;
        }
        self.clamp(t);
        self.want = None;
        if let Some(b) = block {
            self.block_insert_finish(t, b);
        }
        self.close_group();
        if !self.replaying {
            let extent = self.insert_extent.take();
            self.finish_change(None, extent);
        }
    }

    // ── Visual mode ──────────────────────────────────────────────────────

    /// (mode, lines, screen columns) for `.`: on one line the width of the
    /// selection, over several the end's screen column (as Vim's redo); a
    /// block's width (`usize::MAX` after `$`).
    fn visual_extent(&self, t: &dyn TextModel) -> (Mode, usize, usize) {
        if self.mode == Mode::VisualBlock {
            let a = self.block_area(t);
            let width = if a.to_end {
                usize::MAX
            } else {
                a.end_vcol - a.start_vcol + 1
            };
            return (Mode::VisualBlock, a.bottom - a.top + 1, width);
        }
        let (a, b) = (self.anchor.min(self.cursor), self.anchor.max(self.cursor));
        let (al, ac) = text::line_col(t, a);
        let (bl, bc) = text::line_col(t, b);
        if al == bl {
            (
                self.mode,
                1,
                self.vcol(t, bl, bc) - self.vcol(t, al, ac) + 1,
            )
        } else {
            (self.mode, bl - al + 1, self.vcol(t, bl, bc))
        }
    }

    fn run_visual(&mut self, t: &mut dyn TextModel, cmd: Command) -> R {
        let linewise = self.mode == Mode::VisualLine;
        let (a, b) = (self.anchor.min(self.cursor), self.anchor.max(self.cursor));
        let (al, _) = text::line_col(t, a);
        let (bl, _) = text::line_col(t, b);
        let count = cmd.count.unwrap_or(1);
        let exit = |s: &mut Self| s.mode = Mode::Normal;
        // A command that ends visual mode records '< and '> first (Vim's
        // end_visual_mode), so its edit moves them.
        let ends = !matches!(
            cmd.action,
            Action::Move(_)
                | Action::SwapEnds(_)
                | Action::Scroll(_)
                | Action::Z(_)
                | Action::Reselect
                | Action::SetMark(_)
        ) && !matches!(cmd.action, Action::Visual(m) if m != self.mode);
        if ends {
            let at = |p: Pos| self.mk(t, text::line_col(t, p.min(t.len_chars())));
            let want = self.want.unwrap_or_else(|| self.virtcol(t));
            self.marks.visual = Some((at(self.anchor), at(self.cursor), self.mode, want));
            self.visual_marked = true;
        }
        let result = if self.mode == Mode::VisualBlock
            && let Some(r) = self.run_block(t, &cmd, exit)
        {
            r
        } else {
            self.run_visual_inner(t, cmd, a, b, al, bl, linewise, count, exit)
        };
        if !matches!(
            self.mode,
            Mode::Visual | Mode::VisualLine | Mode::VisualBlock | Mode::Insert | Mode::Replace
        ) && !std::mem::take(&mut self.keep_cursor)
        {
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
            Action::Scroll(s) => self.scroll(t, s, cmd.count),
            Action::Z(c) => self.z(t, c, cmd.count),
            Action::AddSub { sub, progressive } => {
                let to_end = self.want == Some(usize::MAX);
                let (ac, bc) = (text::line_col(t, a).1, text::line_col(t, b).1);
                // The cursor goes to the selection's start (a line
                // selection's first column), as for other operators.
                let start = if linewise { t.line_to_char(al) } else { a };
                exit(self);
                // (Undo comes back there too.)
                self.cursor = start;
                let result = self.add_sub_visual(
                    t,
                    sub,
                    count,
                    progressive,
                    (al, ac),
                    (bl, bc),
                    linewise,
                    to_end,
                    None,
                );
                self.cursor = start;
                self.clamp(t);
                self.want = None;
                result
            }
            Action::Reselect => {
                // Swap with the last selection (Vim's nv_gv_cmd).
                let Some((a, b, mode, want)) = self.marks.visual else {
                    return Err(Beep);
                };
                let at = |p: (usize, usize)| {
                    let l = p.0.min(last_line(t));
                    let (l, c) = self.unmk(t, (l, p.1));
                    text::pos(t, l, c.min(line_len(t, l)))
                };
                let now = (
                    self.mk(t, text::line_col(t, self.anchor)),
                    self.mk(t, text::line_col(t, self.cursor)),
                    self.mode,
                    self.want.unwrap_or_else(|| self.virtcol(t)),
                );
                let (a, b) = (at(a), at(b));
                self.marks.visual = Some(now);
                self.anchor = a;
                self.cursor = b;
                self.mode = mode;
                self.want = Some(want);
                Ok(())
            }
            Action::Record(c) => self.start_recording(c),
            Action::SubRepeat(_) | Action::ExRepeat | Action::Jump(_) | Action::Execute(_) => {
                Err(Beep)
            }
            Action::SetMark(c) => {
                if self.set_mark(t, c) {
                    Ok(())
                } else {
                    Err(Beep)
                }
            }
            Action::Move(Motion::Object(obj, around)) => {
                let vis = Vis {
                    anchor: {
                        let (l, c) = text::line_col(t, self.anchor);
                        Cur::new(l, c)
                    },
                    linewise,
                };
                self.want = None;
                let o = match self.object(t, obj, around, count, Some(vis)) {
                    Ok(o) => o,
                    Err(stop) => {
                        self.cursor = Self::at(t, stop.cursor);
                        if let Some(a) = stop.anchor {
                            self.anchor = Self::at(t, a);
                        }
                        return Err(Beep);
                    }
                };
                self.anchor = Self::at(t, o.start);
                let end = Self::at(t, o.end);
                self.cursor = match obj {
                    // Quotes give the operator's range, not a selection.
                    Obj::Quote(_) if !o.inclusive => end.saturating_sub(1),
                    _ => end,
                };
                self.mode = if o.linewise {
                    Mode::VisualLine
                } else {
                    Mode::Visual
                };
                Ok(())
            }
            Action::Move(m) => {
                // In visual mode the cursor can rest on a line's end (after
                // j/k from a longer line, or `$`), selecting its line break.
                let (to, _) = self.motion(t, m, cmd.count, false)?;
                self.cursor = to;
                if m == Motion::LineEnd {
                    let (l, _) = self.lc(t);
                    self.cursor = text::pos(t, l, line_len(t, l));
                }
                Ok(())
            }
            Action::Operate(op, None) | Action::Operate(op, Some(_)) => {
                let lines = linewise || matches!(cmd.action, Action::Operate(_, None));
                let keep = self.want.unwrap_or_else(|| {
                    let (l, c) = text::line_col(t, self.cursor);
                    self.vcol(t, l, c)
                });
                // Vim's start: the anchor (in its line's first column for a
                // line selection), or the cursor if that's before it.
                let anchor = if linewise {
                    t.line_to_char(t.char_to_line(self.anchor.min(t.len_chars())))
                } else {
                    self.anchor
                };
                let start = anchor.min(self.cursor);
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
                    // A visual yank leaves the cursor at the start.
                    self.cursor = start;
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
            Action::Put(keep) => {
                // Even an empty register replaces the selection (with nothing).
                let reg = self.read_register().unwrap_or_default();
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
                let put = put.repeat(count);
                if linewise {
                    // (Marks go as the lines are deleted, then the new put.)
                    if put == old {
                        let n = put.matches('\n').count() + 1;
                        self.marks.lines_replaced(al, bl, n);
                    }
                    self.edit_hint = Some(EditHint::ReplaceLines(al, bl));
                }
                self.edit(t, start..end, &put);
                // The replaced text is deleted into the usual registers.
                self.reg_name = None;
                if !keep {
                    self.store(
                        if linewise { format!("{old}\n") } else { old },
                        linewise,
                        true,
                    );
                }
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
            Action::Visual(mode) => {
                if mode == self.mode {
                    exit(self);
                } else {
                    self.mode = mode;
                }
                Ok(())
            }
            Action::OperateToEnd(op) => self.run_visual_inner(
                t,
                Command {
                    action: Action::Operate(op, None),
                    ..cmd
                },
                a,
                b,
                al,
                bl,
                linewise,
                count,
                exit,
            ),
            Action::SwapEnds(_) => {
                std::mem::swap(&mut self.anchor, &mut self.cursor);
                self.want = None;
                Ok(())
            }
            Action::Cancel => {
                // Leaving visual mode, the column aimed for is the cursor's
                // again (Vim's nv_esc).
                exit(self);
                self.want = None;
                Ok(())
            }
            // `I` and `A` (linewise, as Vim's v_visop makes them): before the
            // first line, or after the selection's last char.
            Action::Insert(kind @ (Insert::LineStart | Insert::LineEnd)) => {
                // The end: the cursor, or the anchor's line start if that's
                // after it (Vim zeroes the anchor's column for a line
                // selection).
                let anchor_start = t.line_to_char(t.char_to_line(self.anchor.min(t.len_chars())));
                let end = if anchor_start < self.cursor {
                    self.cursor
                } else {
                    anchor_start
                };
                exit(self);
                if kind == Insert::LineStart {
                    self.cursor = t.line_to_char(al);
                } else {
                    // After the end's char (unless it's a one-cell char at the
                    // line's start, as the start's column).
                    let (l, c) = text::line_col(t, end.min(t.len_chars()));
                    let end_vcol = self.vcols(t, l, c).1;
                    let len = line_len(t, l);
                    let c = c.min(len.saturating_sub(1));
                    self.cursor = text::pos(t, l, if len > 0 && end_vcol != 0 { c + 1 } else { c });
                }
                self.want = None;
                self.start_insert(t, Insert::Before, count)
            }
            Action::Insert(_) | Action::Replace | Action::Undo | Action::Redo | Action::Repeat => {
                exit(self);
                Err(Beep)
            }
        }
    }

    // ── Undo and repeat ──────────────────────────────────────────────────

    /// An undo step that changes nothing (Vim saved the line for undo, and
    /// nothing changed), at `at`.
    fn noop_undo_step(&mut self, t: &dyn TextModel, at: Pos) {
        self.begin_group();
        let line = t.char_to_line(at.min(t.len_chars()));
        if let Some(g) = self.group.as_mut() {
            g.edits.push(Edit {
                at,
                removed: String::new(),
                inserted: String::new(),
                line,
                spanned: true,
                empty_buf: false,
            });
        }
    }

    /// Where undoing the change under way puts the cursor, if not where
    /// it was before it.
    fn set_undo_cursor(&mut self, pos: Pos) {
        if let Some(g) = self.group.as_mut()
            && g.edits.is_empty()
        {
            g.cursor_before = pos;
        }
    }

    fn begin_group(&mut self) {
        if self.group.is_none() {
            self.group = Some(Group {
                edits: Vec::new(),
                cursor_before: self.cursor,
                marks_before: self.marks.named.clone(),
                top: None,
            });
        }
    }

    fn close_group(&mut self) {
        // A macro's changes are one undo step (Vim doesn't sync undo for
        // keys that weren't typed).
        if self.macro_depth > 0 {
            return;
        }
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
            self.edit_hint = None;
            return;
        }
        let line = t.char_to_line(range.start.min(t.len_chars()));
        let spanned = matches!(self.edit_hint, Some(EditHint::Spanned | EditHint::Join));
        let cursor = self.cursor;
        // Deleting every line leaves Vim's buffer empty (ML_EMPTY), not
        // one empty line.
        let emptied = matches!(self.edit_hint, Some(EditHint::Lines(0, l)) if l == last_line(t));
        let empty_buf = emptied || self.buffer_empty;
        self.buffer_empty = emptied;
        self.changes += 1;
        self.replace_text(t, range.clone(), with);
        self.begin_group();
        if let Some(g) = self.group.as_mut() {
            // Under `:g`, undo comes back to where its first change was made.
            if self.global_busy && g.edits.is_empty() {
                g.cursor_before = cursor;
            }
            g.edits.push(Edit {
                at: range.start,
                removed,
                inserted: with.to_string(),
                line,
                spanned,
                empty_buf,
            });
        }
    }

    /// Undo and redo put back the lowercase marks set before the change,
    /// keeping the ones now for going the other way (Vim's u_undoredo).
    fn swap_marks(&mut self, g: &mut Group) {
        let now: std::collections::HashMap<char, (usize, usize)> = self
            .marks
            .named
            .iter()
            .filter(|(c, _)| c.is_ascii_lowercase())
            .map(|(c, p)| (*c, *p))
            .collect();
        for (c, p) in &g.marks_before {
            if c.is_ascii_lowercase() {
                self.marks.named.insert(*c, *p);
            }
        }
        g.marks_before = now;
    }

    /// Undo or redo one edit: the text, the view, and the marks as Vim's
    /// undo moves them (by whole lines; '[ '] and '. to the lines).
    fn undo_text(
        &mut self,
        t: &mut dyn TextModel,
        range: std::ops::Range<Pos>,
        with: &str,
        spanned: bool,
        empty_buf: bool,
    ) {
        let (l1, c1) = text::line_col(t, range.start.min(t.len_chars()));
        let (l2, _) = text::line_col(t, range.end.min(t.len_chars()));
        let current = t.slice(range.clone());
        // Whole lines (at a line's start, ending in a line break) or the
        // lines the change is in.
        let whole = !spanned
            && c1 == 0
            && (current.is_empty() || current.ends_with('\n'))
            && (with.is_empty() || with.ends_with('\n'));
        // Or whole lines after this one (the last lines, with the line break
        // before them).
        let after = !whole
            && !spanned
            && c1 == line_len(t, l1)
            && (current.is_empty() || current.starts_with('\n'))
            && (with.is_empty() || with.starts_with('\n'));
        let (at, old, new) = if whole {
            (
                l1,
                current.matches('\n').count(),
                with.matches('\n').count(),
            )
        } else if after {
            (
                l1 + 1,
                current.matches('\n').count(),
                with.matches('\n').count(),
            )
        } else {
            (l1, l2 - l1 + 1, with.matches('\n').count() + 1)
        };
        let lines_before = t.len_lines();
        t.replace(range, with);
        // (As Vim's u_undoredo tells it: the lines replaced, and the new.)
        self.changed_lines(t, at, at + old, at as isize + new as isize - 1);
        // To or from an empty buffer, the whole buffer is replaced.
        self.buffer_empty = empty_buf && t.len_chars() == 0;
        let (at, old, new, after) = if empty_buf {
            (0, lines_before, t.len_lines(), false)
        } else {
            (at, old, new, after)
        };
        self.marks_after_undo(at, old, new);
        // ('[ for lines at the end is the one before them.)
        let l1 = if after { at - 1 } else { at };
        let last = last_line(t);
        let end = if new == 0 { at } else { at + new - 1 };
        let start = self.marks.op_start.map_or(l1, |(l, _)| l.min(l1));
        let finish = self.marks.op_end.map_or(end, |(l, _)| l.max(end));
        self.marks.op_start = Some((start.min(last), 0));
        self.marks.op_end = Some((finish.min(last), 0));
        self.marks.last_change = Some((at, 0));
    }

    /// Change the text, and keep the view and the marks on it.
    fn replace_text(&mut self, t: &mut dyn TextModel, range: std::ops::Range<Pos>, with: &str) {
        let (l1, c1) = text::line_col(t, range.start.min(t.len_chars()));
        let (l2, c2) = text::line_col(t, range.end.min(t.len_chars()));
        // (In bytes, as the marks are.)
        let bytes = |l: usize, c: usize| {
            text::line_text(t, l)
                .chars()
                .take(c)
                .map(char::len_utf8)
                .sum::<usize>()
        };
        let (c1, c2) = (bytes(l1, c1), bytes(l2, c2));
        let (len1, len2) = (bytes(l1, usize::MAX), bytes(l2, usize::MAX));
        let removed = t.slice(range.clone());
        // The lines changed (from, and up to but not including), and the
        // last line after the change, as Vim's operations tell the view.
        let breaks = with.matches('\n').count();
        let (lnum, lnume, last_after) = match self.edit_hint {
            Some(EditHint::Lines(first, last)) => (first, last + 1, first as isize - 1),
            _ => (l1, l2 + 1, (l1 + breaks) as isize),
        };
        t.replace(range, with);
        self.changed_lines(t, lnum, lnume, last_after);
        self.marks_after_edit(l1, c1, l2, c2, len1, len2, &removed, with);
    }

    fn undo(&mut self, t: &mut dyn TextModel, count: usize) -> R {
        for i in 0..count {
            let Some(mut g) = self.undo.pop() else {
                // "Already at oldest change": a beep, unless some were undone.
                return if i == 0 { Err(Beep) } else { Ok(()) };
            };
            self.setpcmark(t);
            self.marks.op_start = None;
            self.marks.op_end = None;
            for e in g.edits.iter().rev() {
                let end = e.at + e.inserted.chars().count();
                self.undo_text(t, e.at..end, &e.removed, e.spanned, e.empty_buf);
            }
            self.changes += 1;
            let line = g
                .edits
                .iter()
                .map(|e| e.line)
                .chain(g.top)
                .min()
                .unwrap_or(0)
                .min(last_line(t));
            // Back where the cursor was, if that's in the lines undone (as
            // Vim's u_undoredo); else the first line changed.
            let bottom = g
                .edits
                .iter()
                .map(|e| {
                    e.line
                        + e.removed
                            .matches('\n')
                            .count()
                            .max(e.inserted.matches('\n').count())
                })
                .max()
                .unwrap_or(line);
            let (bl, bc) = text::line_col(t, g.cursor_before.min(t.len_chars()));
            self.cursor = if bl == line || (bl > line && bl <= bottom) {
                text::pos(t, bl, bc.min(line_len(t, bl)))
            } else {
                t.line_to_char(line)
            };
            self.swap_marks(&mut g);
            self.redo.push(g);
        }
        self.clamp(t);
        self.want = None;
        Ok(())
    }

    fn redo(&mut self, t: &mut dyn TextModel, count: usize) -> R {
        for _ in 0..count {
            let Some(mut g) = self.redo.pop() else {
                return Ok(());
            };
            self.setpcmark(t);
            self.marks.op_start = None;
            self.marks.op_end = None;
            for e in &g.edits {
                let end = e.at + e.removed.chars().count();
                self.undo_text(t, e.at..end, &e.inserted, e.spanned, e.empty_buf);
            }
            self.changes += 1;
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
            self.swap_marks(&mut g);
            self.undo.push(g);
        }
        self.clamp(t);
        self.want = None;
        Ok(())
    }

    fn repeat(&mut self, t: &mut dyn TextModel, count: Option<usize>) -> R {
        // A numbered register moves on to the next (`"1p...` puts "1, "2, "3).
        let next = |r: &mut Option<char>| {
            if let Some(c @ '1'..='8') = *r {
                *r = char::from_u32(c as u32 + 1);
            }
        };
        match &mut self.last_change {
            Some(LastChange::Keys { register, .. } | LastChange::Visual { register, .. }) => {
                next(register)
            }
            None => {}
        }
        let Some(last) = self.last_change.clone() else {
            return Ok(());
        };
        let replaying = std::mem::replace(&mut self.replaying, true);
        let result = (|| -> R {
            match &last {
                LastChange::Keys {
                    count: orig,
                    register,
                    keys,
                } => {
                    let n = count.or(*orig);
                    let mut all = register_keys(*register);
                    all.extend(
                        n.map(|n| n.to_string().chars().map(Key::Char).collect::<Vec<_>>())
                            .unwrap_or_default(),
                    );
                    all.extend(keys.iter().copied());
                    for k in all {
                        self.key(t, k)?;
                    }
                }
                LastChange::Visual {
                    mode,
                    lines,
                    last_col_or_chars,
                    register,
                    keys,
                } => {
                    let linewise = *mode == Mode::VisualLine;
                    let (line, col) = self.lc(t);
                    self.anchor = self.cursor;
                    let end_line = (line + lines - 1).min(last_line(t));
                    self.cursor = if *mode == Mode::VisualBlock {
                        // The same block from here (Vim's redo_VIsual).
                        if *last_col_or_chars == usize::MAX {
                            self.want = Some(usize::MAX);
                            text::pos(t, end_line, line_len(t, end_line))
                        } else {
                            self.redo_block_width = Some(*last_col_or_chars);
                            let v = text::vcol(t, line, col, self.tabstop) + last_col_or_chars - 1;
                            let c = text::col_at_vcol(t, end_line, v, self.tabstop);
                            text::pos(t, end_line, c.min(line_len(t, end_line)))
                        }
                    } else if *lines == 1 && !linewise {
                        let v = self.vcol(t, line, col) + last_col_or_chars - 1;
                        text::pos(
                            t,
                            line,
                            self.col_at(t, line, v)
                                .min(line_len(t, line).saturating_sub(1)),
                        )
                    } else {
                        text::pos(t, end_line, self.col_at(t, end_line, *last_col_or_chars))
                    };
                    self.mode = *mode;
                    // The operator is the keys' last command.
                    let mut op = register_keys(*register);
                    op.extend(keys.iter().copied().skip_while(|k| !is_operator_key(*k)));
                    for k in op {
                        self.key(t, k)?;
                    }
                }
            }
            Ok(())
        })();
        self.replaying = replaying;
        self.redo_block_width = None;
        if let (Some(n), Some(LastChange::Keys { count: c, .. })) = (count, &mut self.last_change) {
            *c = Some(n);
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
            block: None,
        }
    }
}

/// `"x` as keys, for replaying a command with its register.
fn register_keys(r: Option<char>) -> Vec<Key> {
    r.map(|r| vec![Key::Char('"'), Key::Char(r)])
        .unwrap_or_default()
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
                | 'I'
                | 'A'
        ) | Key::Ctrl('a' | 'x')
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
        | Action::SwapEnds(_)
        | Action::Scroll(_)
        | Action::Z(_)
        | Action::Record(_)
        | Action::Execute(_) => false,
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

/// The word `*` searches for, on the cursor's line: the keyword under or
/// after the cursor, else the non-blank word under or after it. Its
/// column and text.
fn ident_at(t: &dyn TextModel, c: Cur) -> Option<(usize, String)> {
    let chars: Vec<char> = line_text(t, c.line).chars().collect();
    let col = c.col.min(chars.len());
    let cls = |i: usize| text::class(Some(chars[i]), false);
    let word = |i: usize, same: &dyn Fn(usize) -> bool| {
        // From its start if the cursor's on it, else from where it starts.
        let mut s = i;
        if i == col {
            while s > 0 && same(s - 1) {
                s -= 1;
            }
        }
        let mut e = i;
        while e < chars.len() && same(e) {
            e += 1;
        }
        (s, chars[s..e].iter().collect::<String>())
    };
    if let Some(i) = (col..chars.len()).find(|&j| cls(j) >= 2) {
        let k = cls(i);
        return Some(word(i, &|j| cls(j) == k));
    }
    let i = (col..chars.len()).find(|&j| !is_blank(chars[j]))?;
    let k = cls(i);
    Some(word(i, &|j| cls(j) == k))
}

/// A line typed after `/` or `?`: the keys it took up to Enter, None if
/// Enter hasn't come yet, or Err if it was cancelled (Esc, or Backspace
/// with nothing left).
fn read_line(keys: &[Key]) -> Option<Result<usize, ()>> {
    let mut edit = LineEdit::default();
    for (i, &k) in keys.iter().enumerate() {
        match edit.key(k, &[], &|_| String::new()) {
            ex::Edited::Typing => {}
            ex::Edited::Done => return Some(Ok(i + 1)),
            ex::Edited::Cancelled => return Some(Err(())),
        }
    }
    None
}

/// Where a search's `/` or `?` is in a command's keys, if it has one.
fn search_start(keys: &[Key]) -> Option<usize> {
    let rest = take_prefix(keys).ok()?.2;
    let mut i = keys.len() - rest.len();
    match (keys.get(i), keys.get(i + 1)) {
        (Some(Key::Char('d' | 'c' | 'y' | '<' | '>')), _) => i += 1,
        (Some(Key::Char('g')), Some(Key::Char('u' | 'U' | '~' | 'q' | 'w'))) => i += 2,
        _ => {}
    }
    let rest = take_count(&keys[i..]).1;
    let i = keys.len() - rest.len();
    matches!(keys.get(i), Some(Key::Char('/' | '?'))).then_some(i)
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
        Key::Char(')') => Motion::SentenceForward,
        Key::Char(c @ ('/' | '?')) => {
            return match read_line(&keys[1..]) {
                None => Ok(None),
                Some(Ok(n)) => Ok(Some((Motion::Search(c == '/'), 1 + n))),
                Some(Err(())) => Err(()),
            };
        }
        Key::Char(c @ ('H' | 'M' | 'L')) => Motion::ScreenLine(c),
        Key::Char(b @ (']' | '[')) => {
            let kind = match keys.get(1) {
                None => return Ok(None),
                Some(Key::Char('f')) => SyntaxObject::Function,
                Some(Key::Char('k')) => SyntaxObject::Class,
                Some(Key::Char('h')) => SyntaxObject::Heading,
                Some(_) => return Err(()),
            };
            return Ok(Some((Motion::SyntaxJump(kind, b == ']'), 2)));
        }
        Key::Char(q @ ('\'' | '`')) => {
            return match keys.get(1) {
                None => Ok(None),
                Some(Key::Char(c)) => Ok(Some((Motion::Mark(*c, q == '`'), 2))),
                Some(_) => Err(()),
            };
        }
        Key::Char('n') => Motion::SearchNext(false),
        Key::Char('N') => Motion::SearchNext(true),
        Key::Char('*') => Motion::Star {
            forward: true,
            whole: true,
        },
        Key::Char('#') => Motion::Star {
            forward: false,
            whole: true,
        },
        Key::Char('(') => Motion::SentenceBack,
        Key::Char(c @ ('i' | 'a')) => {
            let obj = match keys.get(1) {
                None => return Ok(None),
                Some(Key::Char(o)) => match o {
                    'w' => Obj::Word(false),
                    'W' => Obj::Word(true),
                    's' => Obj::Sentence,
                    'p' => Obj::Paragraph,
                    '(' | ')' | 'b' => Obj::Block('(', ')'),
                    '{' | '}' | 'B' => Obj::Block('{', '}'),
                    '[' | ']' => Obj::Block('[', ']'),
                    '<' | '>' => Obj::Block('<', '>'),
                    '"' | '\'' | '`' => Obj::Quote(*o),
                    't' => Obj::Tag,
                    'f' => Obj::Syntax(SyntaxObject::Function),
                    'k' => Obj::Syntax(SyntaxObject::Class),
                    'a' => Obj::Syntax(SyntaxObject::Parameter),
                    '/' => Obj::Syntax(SyntaxObject::Comment),
                    '*' => Obj::Syntax(SyntaxObject::Emphasis),
                    'l' => Obj::Syntax(SyntaxObject::Link),
                    'h' => Obj::Syntax(SyntaxObject::Heading),
                    'c' => Obj::Syntax(SyntaxObject::Code),
                    _ => return Err(()),
                },
                Some(_) => return Err(()),
            };
            return Ok(Some((Motion::Object(obj, c == 'a'), 2)));
        }
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
                Some(Key::Char(c @ ('j' | 'k' | '0' | '^' | 'm' | '$'))) => {
                    Ok(Some((Motion::Screen(*c), 2)))
                }
                Some(Key::Down) => Ok(Some((Motion::Screen('j'), 2))),
                Some(Key::Up) => Ok(Some((Motion::Screen('k'), 2))),
                Some(Key::Home) => Ok(Some((Motion::Screen('0'), 2))),
                Some(Key::End) => Ok(Some((Motion::Screen('$'), 2))),
                Some(Key::Char('*')) => Ok(Some((
                    Motion::Star {
                        forward: true,
                        whole: false,
                    },
                    2,
                ))),
                Some(Key::Char('#')) => Ok(Some((
                    Motion::Star {
                        forward: false,
                        whole: false,
                    },
                    2,
                ))),
                Some(_) => Err(()),
            };
        }
        _ => return Err(()),
    };
    Ok(Some((m, 1)))
}

/// The counts and `"x` register names before a command: the count (all of
/// them multiplied), the last register named, and the keys after.
/// A command's count, register and the keys after them.
type Prefix<'a> = (Option<usize>, Option<char>, &'a [Key]);

fn take_prefix(keys: &[Key]) -> Result<Prefix<'_>, Parse> {
    let (mut count, mut rest) = take_count(keys);
    let mut register = None;
    while rest.first() == Some(&Key::Char('"')) {
        match rest.get(1) {
            None => return Err(Parse::Incomplete),
            Some(Key::Char(c)) if readable(*c) => register = Some(*c),
            Some(_) => return Err(Parse::Invalid),
        }
        let (n, after) = take_count(&rest[2..]);
        count = multiply(count, n);
        rest = after;
    }
    Ok((count, register, rest))
}

fn parse(keys: &[Key], visual: bool) -> Parse {
    let (count, register, rest) = match take_prefix(keys) {
        Ok(p) => p,
        Err(p) => return p,
    };
    let Some(&first) = rest.first() else {
        return Parse::Incomplete;
    };
    let done = |action| {
        Parse::Done(Command {
            count,
            register,
            action,
            pattern: None,
        })
    };
    // Scrolling, the same in normal and visual mode.
    let scroll = match first {
        Key::Ctrl('e') => Some(Scroll::Line(true)),
        Key::Ctrl('y') => Some(Scroll::Line(false)),
        Key::Ctrl('d') => Some(Scroll::Half(true)),
        Key::Ctrl('u') => Some(Scroll::Half(false)),
        Key::Ctrl('f') | Key::PageDown => Some(Scroll::Page(true)),
        Key::Ctrl('b') | Key::PageUp => Some(Scroll::Page(false)),
        _ => None,
    };
    if let Some(s) = scroll {
        return done(Action::Scroll(s));
    }
    if first == Key::Char('z') {
        return match rest.get(1) {
            None => Parse::Incomplete,
            Some(Key::Char(c)) if "tz.b-+^".contains(*c) => done(Action::Z(*c)),
            Some(Key::Enter) => done(Action::Z('\r')),
            Some(_) => Parse::Invalid,
        };
    }
    match (first, rest.get(1)) {
        (Key::Char('q'), None) => return Parse::Incomplete,
        (Key::Char('q'), Some(Key::Char(c))) => return done(Action::Record(*c)),
        (Key::Char('q'), Some(_)) => return Parse::Invalid,
        _ => {}
    }
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
                Some(Key::Char('q')) => Some(Some(Op::Format)),
                Some(Key::Char('w')) => Some(Some(Op::FormatKeep)),
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
            Key::Char('X') => Some(Action::Operate(Op::Delete, None)),
            Key::Char('D') => Some(Action::OperateToEnd(Op::Delete)),
            Key::Char('c' | 's') => Some(Action::Operate(Op::Change, Some(Motion::Right))),
            Key::Char('S' | 'R') => Some(Action::Operate(Op::Change, None)),
            Key::Char('I') => Some(Action::Insert(Insert::LineStart)),
            Key::Char('A') => Some(Action::Insert(Insert::LineEnd)),
            Key::Char('C') => Some(Action::OperateToEnd(Op::Change)),
            Key::Char('y') => Some(Action::Operate(Op::Yank, Some(Motion::Right))),
            Key::Char('Y') => Some(Action::Operate(Op::Yank, None)),
            Key::Char('>') => Some(Action::Operate(Op::ShiftRight, None)),
            Key::Char('<') => Some(Action::Operate(Op::ShiftLeft, None)),
            Key::Char('~') => Some(Action::ToggleCase),
            Key::Char('u') => Some(Action::Operate(Op::Lower, Some(Motion::Right))),
            Key::Char('U') => Some(Action::Operate(Op::Upper, Some(Motion::Right))),
            Key::Char('J') => Some(Action::Join(true)),
            // (`P` keeps the registers: what it replaces goes nowhere.)
            Key::Char('p') => Some(Action::Put(false)),
            Key::Char('P') => Some(Action::Put(true)),
            Key::Char('o') => Some(Action::SwapEnds(false)),
            Key::Char('O') => Some(Action::SwapEnds(true)),
            Key::Char('v') => Some(Action::Visual(Mode::Visual)),
            Key::Char('V') => Some(Action::Visual(Mode::VisualLine)),
            Key::Ctrl('v' | 'q') => Some(Action::Visual(Mode::VisualBlock)),
            Key::Esc | Key::Ctrl('c') => Some(Action::Cancel),
            Key::Ctrl('a') => Some(Action::AddSub {
                sub: false,
                progressive: false,
            }),
            Key::Ctrl('x') => Some(Action::AddSub {
                sub: true,
                progressive: false,
            }),
            _ => None,
        };
        if let Some(a) = act {
            return done(a);
        }
        match (first, rest.get(1)) {
            (Key::Char('r'), None) => return Parse::Incomplete,
            (Key::Char('r'), Some(Key::Char(c))) => return done(Action::ReplaceChar(*c)),
            (Key::Char('m'), None) => return Parse::Incomplete,
            (Key::Char('m'), Some(Key::Char(c))) => return done(Action::SetMark(*c)),
            (Key::Char('g'), Some(Key::Char('v'))) => return done(Action::Reselect),
            (Key::Char('g'), Some(Key::Char('J'))) => return done(Action::Join(false)),
            (Key::Char('g'), Some(Key::Char('u'))) => {
                return done(Action::Operate(Op::Lower, Some(Motion::Right)));
            }
            (Key::Char('g'), Some(Key::Char('U'))) => {
                return done(Action::Operate(Op::Upper, Some(Motion::Right)));
            }
            (Key::Char('g'), Some(Key::Char('~'))) => return done(Action::ToggleCase),
            (Key::Char('g'), Some(Key::Char('q'))) => {
                return done(Action::Operate(Op::Format, None));
            }
            (Key::Char('g'), Some(Key::Char('w'))) => {
                return done(Action::Operate(Op::FormatKeep, None));
            }
            (Key::Char('g'), Some(Key::Ctrl(c @ ('a' | 'x')))) => {
                return done(Action::AddSub {
                    sub: *c == 'x',
                    progressive: true,
                });
            }
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
                    register,
                    action: Action::Operate(o, None),
                    pattern: None,
                });
            }
            return match parse_motion(motion_keys) {
                Ok(None) => Parse::Incomplete,
                Ok(Some((m, _))) => Parse::Done(Command {
                    count,
                    register,
                    action: Action::Operate(o, Some(m)),
                    pattern: None,
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
        Key::Char('&') => Some(Action::SubRepeat(false)),
        Key::Ctrl('o') => Some(Action::Jump(false)),
        Key::Tab | Key::Ctrl('i') => Some(Action::Jump(true)),
        Key::Char('p') => Some(Action::Put(false)),
        Key::Char('P') => Some(Action::Put(true)),
        Key::Char('v') => Some(Action::Visual(Mode::Visual)),
        Key::Char('V') => Some(Action::Visual(Mode::VisualLine)),
        Key::Ctrl('v' | 'q') => Some(Action::Visual(Mode::VisualBlock)),
        Key::Char('u') => Some(Action::Undo),
        Key::Ctrl('r') => Some(Action::Redo),
        Key::Char('.') => Some(Action::Repeat),
        Key::Char('Q') => Some(Action::Execute(None)),
        Key::Ctrl('a') => Some(Action::AddSub {
            sub: false,
            progressive: false,
        }),
        Key::Ctrl('x') => Some(Action::AddSub {
            sub: true,
            progressive: false,
        }),
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
        (Key::Char('g'), Some(Key::Char('&'))) => return done(Action::SubRepeat(true)),
        (Key::Char('m'), None) => return Parse::Incomplete,
        (Key::Char('m'), Some(Key::Char(c))) => return done(Action::SetMark(*c)),
        (Key::Char('m'), Some(_)) => return Parse::Invalid,
        (Key::Char('g'), Some(Key::Char('v'))) => return done(Action::Reselect),
        (Key::Char('@'), None) => return Parse::Incomplete,
        (Key::Char('@'), Some(Key::Char(':'))) => return done(Action::ExRepeat),
        (Key::Char('@'), Some(Key::Char(c))) => return done(Action::Execute(Some(*c))),
        (Key::Char('@'), Some(_)) => return Parse::Invalid,
        (Key::Char('g'), Some(Key::Char('J'))) => return done(Action::Join(false)),
        _ => {}
    }
    match parse_motion(rest) {
        Ok(None) => Parse::Incomplete,
        Ok(Some((m, _))) => done(Action::Move(m)),
        Err(()) => Parse::Invalid,
    }
}

/// The line (up to `col`) ends in an open bracket, or a `:` where that
/// opens a block, outside strings and comments.
fn opens_block(t: &dyn TextModel, line: usize, col: usize, smart: &Indenting) -> bool {
    let text: Vec<char> = line_text(t, line).chars().take(col).collect();
    let Some(last) = text.iter().rposition(|&c| !is_blank(c)) else {
        return false;
    };
    let opener = matches!(text[last], '{' | '[' | '(') || (smart.colon && text[last] == ':');
    opener && t.syntax_region(text::pos(t, line, last)).is_none()
}

/// The line starts with a closing bracket.
fn closes_block(t: &dyn TextModel, line: usize) -> bool {
    let first = first_non_blank(t, line);
    matches!(char_at(t, line, first), Some('}' | ']' | ')'))
        && t.syntax_region(text::pos(t, line, first)).is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::parse;
    use ropey::Rope;

    fn type_keys(vim: &mut Vim, t: &mut Rope, keys: &str) {
        for k in parse(keys) {
            let _ = vim.key(t, k);
        }
    }

    #[test]
    fn clipboard_registers_go_out_and_come_in() {
        let mut t = Rope::from_str("one two\nthree");
        let mut vim = Vim::new();
        type_keys(&mut vim, &mut t, "\"+yiw");
        assert_eq!(vim.take_clipboard(), Some(('+', "one".into())));
        assert_eq!(vim.take_clipboard(), None);
        type_keys(&mut vim, &mut t, "\"*yy");
        assert_eq!(vim.take_clipboard(), Some(('*', "one two\n".into())));
        // Pasted from outside: a trailing line break makes it whole lines.
        vim.set_clipboard('+', "pasted\n");
        type_keys(&mut vim, &mut t, "\"+p");
        assert_eq!(t.to_string(), "one two\npasted\nthree");
        assert!(!vim.naming_register());
        type_keys(&mut vim, &mut t, "\"");
        assert!(vim.naming_register());
    }

    #[test]
    fn read_only_registers_hold_the_insert_the_command_and_the_file() {
        let mut t = Rope::from_str("x");
        let mut vim = Vim::new();
        vim.set_file_name(Some("notes.md"));
        type_keys(&mut vim, &mut t, "ione<CR>two<Esc>:3<CR>");
        assert_eq!(vim.get_register('.').text, "one\ntwo");
        assert_eq!(vim.get_register(':').text, "3");
        assert_eq!(vim.get_register('%').text, "notes.md");
        // They can be put, but not written.
        assert_eq!(vim.key(&mut t, Key::Char('"')), Ok(()));
        assert_eq!(vim.key(&mut t, Key::Char('%')), Ok(()));
        assert_eq!(vim.key(&mut t, Key::Char('y')), Ok(()));
        assert_eq!(vim.key(&mut t, Key::Char('y')), Err(Beep));
        type_keys(&mut vim, &mut t, "gg0\"%P");
        assert_eq!(text::line_col(&t, 0), (0, 0));
        assert!(t.to_string().starts_with("notes.md"), "{:?}", t.to_string());
    }

    #[test]
    fn registers_outlive_the_text() {
        let mut t = Rope::from_str("keep me\nand this");
        let mut vim = Vim::new();
        type_keys(&mut vim, &mut t, "\"ayyjdd");
        let mut other = Rope::from_str("new file");
        let mut vim = vim.for_other_text();
        assert_eq!(vim.cursor(), 0);
        type_keys(&mut vim, &mut other, "\"ap\"1p");
        assert_eq!(other.to_string(), "new file\nkeep me\nand this");
    }

    #[test]
    fn ctrl_r_inserts_a_register_and_dot_repeats_the_text() {
        let mut t = Rope::from_str("word\n");
        let mut vim = Vim::new();
        type_keys(&mut vim, &mut t, "yiwA <C-r>0!<Esc>");
        assert_eq!(t.to_string(), "word word!\n");
        // `.` types the same text again, even though "0 has changed.
        type_keys(&mut vim, &mut t, "jyyk.");
        assert_eq!(t.to_string(), "word word! word!\n");
    }

    #[test]
    fn a_search_is_typed_shown_and_highlighted() {
        let mut t = Rope::from_str("one two\ntwo one\nthree");
        let mut vim = Vim::new();
        type_keys(&mut vim, &mut t, "/tw");
        assert_eq!(vim.mode(), Mode::CommandLine);
        assert_eq!(vim.command_line().as_deref(), Some("/tw"));
        // While typing: where it would go, and every match.
        assert_eq!(vim.search_preview(&t), Some(4..6));
        assert_eq!(vim.search_highlights(&t, 0..3), [4..6, 8..10]);
        type_keys(&mut vim, &mut t, "o<CR>");
        assert_eq!((vim.mode(), vim.cursor()), (Mode::Normal, 4));
        assert_eq!(vim.search_preview(&t), None);
        assert_eq!(vim.search_highlights(&t, 0..3), [4..7, 8..11]);
        assert_eq!(vim.get_register('/').text, "two");
        type_keys(&mut vim, &mut t, "n");
        assert_eq!(vim.take_message(), None);
        type_keys(&mut vim, &mut t, "n");
        assert_eq!(vim.cursor(), 4);
        assert_eq!(
            vim.take_message().as_deref(),
            Some("search hit BOTTOM, continuing at TOP")
        );
        // :noh hides the matches until the next search.
        type_keys(&mut vim, &mut t, ":noh<CR>");
        assert!(vim.search_highlights(&t, 0..3).is_empty());
        type_keys(&mut vim, &mut t, "n");
        assert_eq!(vim.search_highlights(&t, 0..3).len(), 2);
    }

    #[test]
    fn a_missing_pattern_says_so() {
        let mut t = Rope::from_str("abc");
        let mut vim = Vim::new();
        type_keys(&mut vim, &mut t, "n");
        assert_eq!(
            vim.take_message().as_deref(),
            Some("E35: No previous regular expression")
        );
        type_keys(&mut vim, &mut t, "/zz<CR>");
        assert_eq!(
            vim.take_message().as_deref(),
            Some("E486: Pattern not found: zz")
        );
        assert_eq!(vim.cursor(), 0);
    }

    #[test]
    fn set_changes_and_reports_options() {
        let mut t = Rope::from_str("x");
        let mut vim = Vim::new();
        type_keys(&mut vim, &mut t, ":set ic ts=4<CR>");
        assert!(vim.ignorecase);
        assert_eq!(vim.tabstop, 4);
        type_keys(&mut vim, &mut t, ":set ic? noic ic? ts?<CR>");
        assert_eq!(
            vim.take_message().as_deref(),
            Some("  ignorecase noignorecase   tabstop=4")
        );
        type_keys(&mut vim, &mut t, ":set bogus<CR>");
        assert_eq!(
            vim.take_message().as_deref(),
            Some("E518: Unknown option: bogus")
        );
    }

    #[test]
    fn the_apps_commands_are_handed_over_with_what_follows() {
        let mut t = Rope::from_str("one\ntwo");
        let mut vim = Vim::new();
        type_keys(&mut vim, &mut t, ":e notes.md<CR>");
        assert_eq!(vim.take_command().as_deref(), Some("e notes.md"));
        // The engine runs its part, then hands over the rest.
        type_keys(&mut vim, &mut t, ":s/one/1/|w|q<CR>");
        assert_eq!(t.to_string(), "1\ntwo");
        assert_eq!(vim.take_command().as_deref(), Some("w|q"));
    }

    #[test]
    fn the_command_line_has_a_cursor() {
        let mut t = Rope::from_str("x");
        let mut vim = Vim::new();
        type_keys(&mut vim, &mut t, ":abc<Left><Left>");
        assert_eq!(vim.command_line_cursor(), Some((":abc".into(), 2)));
        type_keys(&mut vim, &mut t, "X<End>");
        assert_eq!(vim.command_line_cursor(), Some((":aXbc".into(), 5)));
        type_keys(&mut vim, &mut t, "<Esc>/fo<Left>");
        assert_eq!(vim.command_line_cursor(), Some(("/fo".into(), 2)));
    }

    #[test]
    fn a_colon_command_is_typed_then_handed_over() {
        let mut t = Rope::from_str("one\ntwo\nthree");
        let mut vim = Vim::new();
        type_keys(&mut vim, &mut t, ":wqx<BS>");
        assert_eq!(vim.mode(), Mode::CommandLine);
        assert_eq!(vim.command_line().as_deref(), Some(":wq"));
        type_keys(&mut vim, &mut t, "<CR>");
        assert_eq!(vim.mode(), Mode::Normal);
        assert_eq!(vim.take_command().as_deref(), Some("wq"));
        assert_eq!(vim.take_command(), None);
    }

    #[test]
    fn a_line_number_is_run_by_the_engine() {
        let mut t = Rope::from_str("one\n  two\nthree");
        let mut vim = Vim::new();
        // It keeps the cursor's column, as Neovim does ('nostartofline').
        type_keys(&mut vim, &mut t, ":2<CR>");
        assert_eq!(text::line_col(&t, vim.cursor()), (1, 0));
        assert_eq!(vim.take_command(), None);
    }

    #[test]
    fn escape_or_backspacing_past_the_colon_cancels() {
        let mut t = Rope::from_str("x");
        let mut vim = Vim::new();
        type_keys(&mut vim, &mut t, ":q<Esc>");
        assert_eq!((vim.mode(), vim.take_command()), (Mode::Normal, None));
        type_keys(&mut vim, &mut t, ":<BS>");
        assert_eq!(vim.mode(), Mode::Normal);
    }
}
