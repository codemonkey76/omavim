//! Omavim: a dead-simple writing app with Vim motions. See PLAN.md.

mod colors;
mod config;
mod document;
mod drafts;
mod editor;
mod help;
mod portal;

use colors::Colors;
use config::{Action, Config};
use document::{Document, Recorder};
use editor::{Editor, KeyPress, View};
use iced::keyboard::{Key as IcedKey, key::Named};
use iced::widget::{column, container, row, space, text};
use iced::{Color, Element, Font, Length, Subscription, Task, Theme};
use omavim_vim::{Key, Mode, Vim};
use portal::Scheme;
use std::path::PathBuf;

const FONT: Font = Font::with_name("iA Writer Mono S");
/// The text size Omavim is designed around (Omawrite's is 12px at its scale),
/// at the desktop's text size of 1.0 (12px in `omarchy display text size`).
const TEXT_SIZE: f32 = 17.0;
/// The footer's.
const SMALL_SIZE: f32 = 13.0;

fn main() -> iced::Result {
    iced::application(App::boot, App::update, App::view)
        .title(App::title)
        .theme(App::theme)
        .subscription(App::subscription)
        .font(include_bytes!("../../../fonts/iAWriterMonoS-Regular.ttf").as_slice())
        .font(include_bytes!("../../../fonts/iAWriterMonoS-Bold.ttf").as_slice())
        .font(include_bytes!("../../../fonts/iAWriterMonoS-Italic.ttf").as_slice())
        .font(include_bytes!("../../../fonts/iAWriterMonoS-BoldItalic.ttf").as_slice())
        .default_font(FONT)
        .window_size((900.0, 1000.0))
        // (Closing asks first, if there are unsaved changes.)
        .exit_on_close_request(false)
        .run()
}

struct App {
    doc: Document,
    vim: Vim,
    scheme: Scheme,
    /// The desktop's text size, as a factor.
    scale: f32,
    /// The colours: the Omarchy theme's, or Omavim's for `scheme`.
    colors: Colors,
    /// A short message in the footer: an error, or what just happened.
    status: Option<String>,
    /// What to do once a save finishes (`:wq` quits after it, say).
    after_save: Option<Then>,
    /// A question in the footer, waiting for its answer.
    prompt: Option<Prompt>,
    /// A save is being written.
    writing: bool,
    /// Where drafts go (None: nowhere, as in tests).
    drafts_dir: Option<PathBuf>,
    /// The text changed since the draft was written: when.
    draft_due: Option<std::time::Instant>,
    /// The file this Omavim last wrote a draft of, to remove it.
    drafted: Option<Option<PathBuf>>,
    /// A parse is running on another thread.
    parsing: bool,
    config: Config,
    /// The leader key was typed: the next key is Omavim's.
    leader_pending: bool,
    /// The key reference is showing.
    help: bool,
    /// The last highlights worked out, and what for (the text's changes
    /// count, the first line, how many), so a redraw that changed none of
    /// that doesn't work them out again.
    highlight_cache: std::cell::RefCell<HighlightCache>,
}

type Highlights = Vec<(std::ops::Range<usize>, colors::Style)>;

#[derive(Default)]
struct HighlightCache {
    /// The text's version, its language, and the lines.
    key: Option<(u64, omavim_syntax::Lang, u64, usize, usize)>,
    highlights: std::rc::Rc<Highlights>,
}

#[derive(Debug, Clone)]
enum Message {
    Key(KeyPress),
    Scheme(Scheme),
    /// The desktop's text size changed.
    TextScale(f32),
    Opened(Result<Option<(PathBuf, String)>, String>),
    /// Where Save As chose to write (None: cancelled).
    SaveTo(Result<Option<PathBuf>, String>),
    Saved(Result<PathBuf, String>),
    /// The window came to the front: the clipboard may have changed.
    Focused,
    /// The desktop clipboard (`'+'`) or primary selection (`'*'`), read.
    Clipboard(char, Option<String>),
    /// The text area's size: cells in a row, and rows.
    Resized(usize, usize),
    /// The mouse wheel: rows to scroll, down if positive.
    Scroll(isize),
    /// Time to see if the Omarchy theme, or the file, changed.
    Tick,
    /// The syntax tree, parsed again off the main thread.
    Parsed(Handoff<omavim_syntax::Parsed>),
    /// The window's close button (or the desktop's close key).
    CloseRequested,
    /// The window went to the back.
    Unfocused,
    /// Time to see if typing's stopped, to write a draft.
    DraftTick,
}

/// A question for a key's answer.
#[derive(Debug, Clone, PartialEq)]
enum Prompt {
    /// Save the unsaved changes before this?
    Save(Then),
    /// The file changed on disk, and here too: keep this, or load that?
    Changed(String),
    /// The file changed on disk since it was read: write over it anyway?
    Overwrite(PathBuf),
    /// A draft left by an Omavim that closed without saving it: take it
    /// back? (And the draft files, to remove after.)
    Recover(drafts::Draft, Vec<PathBuf>),
}

/// What to do after dealing with unsaved changes.
#[derive(Debug, Clone, PartialEq)]
enum Then {
    Quit,
    /// The open picker.
    Open,
    /// `:confirm e [file]`.
    Edit(Option<String>),
}

/// Something made on another thread, handed over in a message (which has
/// to be cloneable): the first to take it has it.
struct Handoff<T>(std::sync::Arc<std::sync::Mutex<Option<T>>>);

impl<T> Clone for Handoff<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> Handoff<T> {
    fn new(value: T) -> Self {
        Self(std::sync::Arc::new(std::sync::Mutex::new(Some(value))))
    }

    fn take(&self) -> Option<T> {
        self.0.lock().ok()?.take()
    }
}

impl<T> std::fmt::Debug for Handoff<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Handoff")
    }
}

impl App {
    fn boot() -> Self {
        // `omavim notes.md`: open it, or start it if it doesn't exist yet.
        let (doc, status) = match std::env::args_os().nth(1).map(PathBuf::from) {
            Some(path) => match std::fs::read_to_string(&path) {
                Ok(contents) => (Document::open(path, &contents), None),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    (Document::open(path, ""), Some("New file".into()))
                }
                Err(e) => (
                    Document::default(),
                    Some(format!("{}: {e}", path.display())),
                ),
            },
            None => (Document::default(), None),
        };
        let (config, config_error) = Config::load();
        let status = config_error.or(status);
        let mut vim = Vim::new();
        // For writing: j and k go by screen line, as gj and gk.
        vim.display_lines = true;
        vim.set_file_name(doc.path.as_deref().and_then(|p| p.to_str()));
        vim.set_filetype(doc.filetype());
        let mut app = Self {
            doc,
            vim,
            scheme: Scheme::default(),
            scale: 1.0,
            colors: Colors::current(Scheme::default()),
            status,
            after_save: None,
            prompt: None,
            writing: false,
            drafts_dir: None,
            draft_due: None,
            drafted: None,
            parsing: false,
            config,
            leader_pending: false,
            help: false,
            highlight_cache: Default::default(),
        };
        app.drafts_dir = drafts::dir();
        app.offer_draft();
        app
    }

    fn set_colors(&mut self, colors: Colors) {
        if colors != self.colors {
            self.colors = colors;
            *self.highlight_cache.borrow_mut() = HighlightCache::default();
        }
    }

    fn title(&self) -> String {
        format!(
            "{}{} — Omavim",
            self.doc.name(),
            if self.doc.dirty { " •" } else { "" }
        )
    }

    fn theme(&self) -> Option<Theme> {
        Some(Theme::custom("Omavim", self.colors.palette()))
    }

    fn subscription(&self) -> Subscription<Message> {
        let drafting = match self.draft_due {
            Some(_) => {
                iced::time::every(std::time::Duration::from_millis(500)).map(|_| Message::DraftTick)
            }
            None => Subscription::none(),
        };
        Subscription::batch([
            drafting,
            Subscription::run(portal::color_scheme).map(Message::Scheme),
            Subscription::run(portal::text_scale).map(Message::TextScale),
            // (A new Omarchy theme swaps its colours file in: cheap to check.)
            iced::time::every(std::time::Duration::from_secs(2)).map(|_| Message::Tick),
            iced::event::listen_with(|event, _, _| match event {
                iced::Event::Window(iced::window::Event::Focused) => Some(Message::Focused),
                iced::Event::Window(iced::window::Event::Unfocused) => Some(Message::Unfocused),
                iced::Event::Window(iced::window::Event::CloseRequested) => {
                    Some(Message::CloseRequested)
                }
                _ => None,
            }),
        ])
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Key(press) => return self.key(press),
            // (Kept to something readable, whatever's set.)
            Message::TextScale(scale) => self.scale = scale.clamp(0.5, 3.0),
            Message::Scheme(scheme) => {
                self.scheme = scheme;
                self.set_colors(Colors::current(scheme));
            }
            Message::Parsed(parsed) => {
                self.parsing = false;
                if let (Some(parsed), Some(syntax)) = (parsed.take(), self.doc.syntax.as_mut()) {
                    syntax.finish(parsed);
                }
                // Edited while it parsed: again.
                return self.parse_later();
            }
            Message::Tick => {
                if Colors::omarchy_stamp() != self.colors.source {
                    self.set_colors(Colors::current(self.scheme));
                }
                return self.check_disk();
            }
            Message::Opened(Ok(Some((path, contents)))) => {
                self.switch_to(Document::open(path, &contents), None);
            }
            Message::Focused => return Task::batch([read_clipboards(), self.check_disk()]),
            Message::Clipboard(register, text) => {
                self.vim
                    .set_clipboard(register, text.as_deref().unwrap_or(""));
            }
            Message::Resized(cells, rows) => self.vim.set_screen(&self.doc.text, cells, rows),
            Message::Scroll(rows) => self.vim.scroll_view(&self.doc.text, rows),
            Message::Opened(Ok(None)) => {}
            Message::SaveTo(Ok(None)) => self.after_save = None,
            Message::SaveTo(Ok(Some(path))) => return self.write(path),
            Message::Saved(Ok(path)) => {
                self.writing = false;
                self.remove_draft();
                self.draft_due = None;
                self.doc.disk = document::disk_stamp(&path);
                self.status = Some(format!("Saved {}", path.display()));
                self.vim.set_file_name(path.to_str());
                self.doc.path = Some(path);
                // (A new name may be a new language.)
                self.doc.detect_language();
                self.vim.set_filetype(self.doc.filetype());
                self.doc.dirty = false;
                if let Some(then) = self.after_save.take() {
                    return self.go(then);
                }
            }
            Message::Opened(Err(e)) | Message::SaveTo(Err(e)) | Message::Saved(Err(e)) => {
                self.writing = false;
                self.after_save = None;
                self.status = Some(e)
            }
            Message::CloseRequested => return self.leave(Then::Quit),
            Message::Unfocused => {
                if self.draft_due.is_some() {
                    self.write_draft();
                }
            }
            Message::DraftTick => {
                if self
                    .draft_due
                    .is_some_and(|t| t.elapsed() >= std::time::Duration::from_millis(1500))
                {
                    self.write_draft();
                }
            }
        }
        Task::none()
    }

    /// Keys go to Vim, except the app's own: Ctrl+S in every mode, and the
    /// leader and the key after it.
    fn key(&mut self, press: KeyPress) -> Task<Message> {
        let m = press.modifiers;
        if m.control()
            && !m.shift()
            && let IcedKey::Character(c) = press.key.as_ref()
            && c.eq_ignore_ascii_case("s")
        {
            return self.save();
        }
        let keys = vim_keys(&press);
        if self.prompt.is_some() {
            return self.prompt_key(&keys);
        }
        if self.help {
            return self.help_key(&keys);
        }
        if std::mem::take(&mut self.leader_pending) {
            // (Anything that isn't a leader key, Esc too, just cancels.)
            return match keys[..] {
                [Key::Char(c)] => match self.config.action(c) {
                    Some(action) => self.leader(action),
                    None => Task::none(),
                },
                _ => Task::none(),
            };
        }
        if keys[..] == [Key::Char(self.config.leader)] && self.leader_ready() {
            self.leader_pending = true;
            return Task::none();
        }
        let before = self.vim.changes();
        let mut text = Recorder::new(&mut self.doc);
        for key in keys {
            if self.vim.key(&mut text, key).is_err() {
                // Vim beeps; the keys after it still count, as typed keys do.
            }
        }
        self.after_vim(before)
    }

    /// Keys while the key reference shows: close it, or scroll.
    fn help_key(&mut self, keys: &[Key]) -> Task<Message> {
        use iced::widget::operation::{AbsoluteOffset, RelativeOffset, scroll_by, snap_to};
        let by = |y: f32| scroll_by(help::ID, AbsoluteOffset { x: 0.0, y });
        match keys {
            [Key::Esc | Key::Enter | Key::Char('q' | '?')] => {
                self.help = false;
                Task::none()
            }
            [Key::Char('j') | Key::Down | Key::Ctrl('e')] => by(40.0),
            [Key::Char('k') | Key::Up | Key::Ctrl('y')] => by(-40.0),
            [Key::Ctrl('d' | 'f') | Key::PageDown | Key::Char(' ')] => by(400.0),
            [Key::Ctrl('u' | 'b') | Key::PageUp] => by(-400.0),
            [Key::Char('g')] => snap_to(help::ID, RelativeOffset::START),
            [Key::Char('G')] => snap_to(help::ID, RelativeOffset::END),
            _ => Task::none(),
        }
    }

    /// The leader key starts one of Omavim's own in normal and visual mode,
    /// when no count, register or operator has been typed.
    fn leader_ready(&self) -> bool {
        matches!(
            self.vim.mode(),
            Mode::Normal | Mode::Visual | Mode::VisualLine
        ) && self.vim.pending().is_empty()
            && self.vim.command_line_cursor().is_none()
    }

    /// One of Omavim's own actions.
    fn leader(&mut self, action: Action) -> Task<Message> {
        match action {
            Action::Save => self.save(),
            Action::SaveAs => self.save_as(),
            Action::Open => self.open(),
            Action::NewWindow => self.new_window(),
            Action::Print => self.print(),
            Action::Fullscreen => toggle_fullscreen(),
            Action::Bold | Action::Italic | Action::Link => {
                let before = self.vim.changes();
                let mut text = Recorder::new(&mut self.doc);
                let _ = match action {
                    Action::Bold => self.vim.surround(&mut text, "**"),
                    Action::Italic => self.vim.surround(&mut text, "_"),
                    _ => self.vim.link(&mut text),
                };
                self.after_vim(before)
            }
            Action::Help => {
                self.help = true;
                Task::none()
            }
            Action::Close => self.leave(Then::Quit),
        }
    }

    /// Another window, as another Omavim.
    fn new_window(&mut self) -> Task<Message> {
        match std::env::current_exe().and_then(|exe| std::process::Command::new(exe).spawn()) {
            Ok(mut child) => {
                // (Reaped when it closes.)
                std::thread::spawn(move || child.wait());
            }
            Err(e) => self.status = Some(format!("New window: {e}")),
        }
        Task::none()
    }

    fn print(&mut self) -> Task<Message> {
        self.status = Some("Printing isn't ready yet".into());
        Task::none()
    }

    /// After keys went to Vim: what they changed, and what Vim handed over
    /// for the app to do.
    fn after_vim(&mut self, before: u64) -> Task<Message> {
        // `:set filetype=`: highlight as that language (plain text for one
        // Omavim doesn't know, as Vim keeps the name).
        if let Some(filetype) = self.vim.take_filetype() {
            self.doc
                .set_language(omavim_syntax::Lang::from_name(&filetype));
        }
        if self.vim.changes() != before {
            self.doc.dirty = true;
            self.status = None;
            self.draft_due = Some(std::time::Instant::now());
        }
        // "search hit BOTTOM, continuing at TOP", "E486: Pattern not found".
        if let Some(message) = self.vim.take_message() {
            self.status = Some(message);
        }
        let mut tasks = vec![self.parse_later()];
        // `"+y` / `"*y`: onto the clipboard or primary selection.
        if let Some((register, text)) = self.vim.take_clipboard() {
            tasks.push(if register == '*' {
                iced::clipboard::write_primary(text)
            } else {
                iced::clipboard::write(text)
            });
        }
        // `"` or Ctrl-R typed: bring `"+` and `"*` up to date before the
        // register's name (typed after) is used.
        if self.vim.naming_register() {
            tasks.push(read_clipboards());
        }
        if let Some(cmd) = self.vim.take_command() {
            tasks.push(self.command(&cmd));
        }
        Task::batch(tasks)
    }

    /// Bring the syntax tree up to date with the edits, on another thread
    /// (drawing meanwhile from the old one, moved to fit the edits).
    fn parse_later(&mut self) -> Task<Message> {
        if self.parsing {
            return Task::none();
        }
        let Some(job) = self
            .doc
            .syntax
            .as_ref()
            .and_then(|s| s.parse_job(&self.doc.text))
        else {
            return Task::none();
        };
        self.parsing = true;
        Task::perform(
            async move {
                let parsed = tokio::task::spawn_blocking(move || job.run()).await;
                parsed.ok().map(Handoff::new)
            },
            |parsed| Message::Parsed(parsed.unwrap_or_else(|| Handoff(Default::default()))),
        )
    }

    /// The syntax highlights for the lines on screen, as char ranges and
    /// how to draw them.
    fn highlights(&self) -> std::rc::Rc<Highlights> {
        let Some(syntax) = &self.doc.syntax else {
            return Default::default();
        };
        let t = &self.doc.text;
        let last = t.len_lines().saturating_sub(1);
        let first = self.vim.top().0.min(last);
        // (A line takes a row at least.)
        let count = self.vim.screen_height().max(1);
        let key = (
            self.vim.changes(),
            syntax.lang(),
            syntax.revision(),
            first,
            count,
        );
        if let Ok(cache) = self.highlight_cache.try_borrow()
            && cache.key == Some(key)
        {
            return cache.highlights.clone();
        }
        let end_line = (first + count).min(last);
        let from = t.line_to_byte(first);
        let to = if end_line == last {
            t.len_bytes()
        } else {
            t.line_to_byte(end_line + 1)
        };
        let highlights: Highlights = syntax
            .highlights(t, from..to)
            .into_iter()
            .map(|s| (s.range, self.colors.style(s.name)))
            .filter(|(_, style)| *style != colors::Style::default())
            .map(|(r, style)| (t.byte_to_char(r.start)..t.byte_to_char(r.end), style))
            .collect();
        let highlights = std::rc::Rc::new(highlights);
        if let Ok(mut cache) = self.highlight_cache.try_borrow_mut() {
            *cache = HighlightCache {
                key: Some(key),
                highlights: highlights.clone(),
            };
        }
        highlights
    }

    /// Commands the engine handed over, separated by `|` (`:w|q`).
    fn command(&mut self, line: &str) -> Task<Message> {
        let mut tasks = Vec::new();
        let mut saving = false;
        for cmd in line.split('|').map(str::trim).filter(|c| !c.is_empty()) {
            // A quit after a write waits for the write to finish.
            if saving && matches!(cmd, "q" | "quit" | "q!" | "quit!" | "qa" | "qa!") {
                self.after_save = Some(Then::Quit);
                continue;
            }
            saving |= matches!(cmd.split_whitespace().next(), Some("w" | "w!" | "write"));
            tasks.push(self.command_one(cmd));
        }
        Task::batch(tasks)
    }

    /// One command the engine handed over: the file and window ones.
    fn command_one(&mut self, cmd: &str) -> Task<Message> {
        let (name, arg) = match cmd.split_once(char::is_whitespace) {
            Some((n, a)) => (n, Some(a.trim()).filter(|a| !a.is_empty())),
            None => (cmd, None),
        };
        match name {
            "e" | "edit" | "e!" | "edit!" => {
                self.edit(arg, name.ends_with('!'));
                Task::none()
            }
            "w" | "write" => match arg {
                Some(file) => self.write(self.resolve(file)),
                None => self.save(),
            },
            // (Over a file changed on disk without asking.)
            "w!" | "write!" => match (arg, self.doc.path.clone()) {
                (Some(file), _) => self.write_now(self.resolve(file)),
                (None, Some(path)) => self.write_now(path),
                (None, None) => self.save_as(),
            },
            "sav" | "saveas" => match arg {
                Some(file) => self.write(self.resolve(file)),
                None => self.save_as(),
            },
            "new" | "vne" | "vnew" | "tabnew" | "tabe" | "tabedit" => self.new_window(),
            "ha" | "hardcopy" => self.print(),
            "fullscreen" => toggle_fullscreen(),
            "h" | "help" => {
                self.help = true;
                Task::none()
            }
            // `:confirm q`, `:confirm e [file]`: ask about unsaved changes
            // rather than refusing.
            "conf" | "confirm" => {
                let rest = arg.unwrap_or("");
                let (what, file) = match rest.split_once(char::is_whitespace) {
                    Some((w, f)) => (w, Some(f.trim().to_string())),
                    None => (rest, None),
                };
                match what {
                    "q" | "quit" | "qa" | "qall" | "clo" | "close" => self.leave(Then::Quit),
                    "e" | "edit" => self.leave(Then::Edit(file)),
                    _ => self.command_one(rest),
                }
            }
            "q" | "quit" | "clo" | "close" if self.doc.dirty => {
                self.status = Some("E37: No write since last change (add ! to override)".into());
                Task::none()
            }
            "q" | "quit" | "q!" | "quit!" | "qa" | "qa!" | "clo" | "close" => self.quit(),
            "x" | "xit" | "exi" | "exit" if !self.doc.dirty => self.quit(),
            "wq" | "x" | "xit" | "exi" | "exit" => {
                self.after_save = Some(Then::Quit);
                match arg {
                    Some(file) => self.write(self.resolve(file)),
                    None => self.save(),
                }
            }
            _ => {
                self.status = Some(format!("E492: Not an editor command: {cmd}"));
                Task::none()
            }
        }
    }

    /// `:e [file]`: open a file (or the current one again); `!` drops
    /// unsaved changes.
    fn edit(&mut self, file: Option<&str>, force: bool) {
        if self.doc.dirty && !force {
            self.status = Some("E37: No write since last change (add ! to override)".into());
            return;
        }
        let path = match (file, &self.doc.path) {
            (Some(f), _) => self.resolve(f),
            (None, Some(p)) => p.clone(),
            (None, None) => {
                self.status = Some("E32: No file name".into());
                return;
            }
        };
        let (doc, status) = match std::fs::read_to_string(&path) {
            Ok(contents) => (Document::open(path, &contents), None),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                (Document::open(path, ""), Some("New file".to_string()))
            }
            Err(e) => {
                self.status = Some(format!("{}: {e}", path.display()));
                return;
            }
        };
        self.switch_to(doc, status);
    }

    /// Another document in place of this one (whose changes have been saved,
    /// or let go, by now).
    fn switch_to(&mut self, doc: Document, status: Option<String>) {
        self.remove_draft();
        self.draft_due = None;
        self.doc = doc;
        *self.highlight_cache.borrow_mut() = HighlightCache::default();
        self.status = status;
        self.vim = self.vim.for_other_text();
        self.vim
            .set_file_name(self.doc.path.as_deref().and_then(|p| p.to_str()));
        self.vim.set_filetype(self.doc.filetype());
        self.offer_draft();
    }

    /// Close cleanly: this Omavim's draft goes (the changes were saved, or
    /// let go).
    fn quit(&mut self) -> Task<Message> {
        self.remove_draft();
        iced::exit()
    }

    /// Write the unsaved text to a draft (or remove it, with nothing unsaved).
    fn write_draft(&mut self) {
        self.draft_due = None;
        let Some(dir) = self.drafts_dir.clone() else {
            return;
        };
        if !self.doc.dirty {
            self.remove_draft();
            return;
        }
        let draft = drafts::Draft {
            path: self.doc.path.clone(),
            pid: std::process::id(),
            written: drafts::now(),
            final_newline: self.doc.final_newline,
            text: self.doc.text.to_string(),
        };
        // A new name since the last draft: that one goes.
        if self.drafted.as_ref().is_some_and(|p| *p != draft.path) {
            self.remove_draft();
        }
        match drafts::write(&dir, &draft) {
            Ok(()) => self.drafted = Some(draft.path),
            Err(e) => self.status = Some(format!("Couldn't write a draft: {e}")),
        }
    }

    fn remove_draft(&mut self) {
        if let (Some(dir), Some(path)) = (&self.drafts_dir, self.drafted.take()) {
            drafts::remove(dir, path.as_deref(), std::process::id());
        }
    }

    /// A draft of this file that an Omavim left behind: offer it back (or
    /// just remove it, if it's what the file has anyway).
    fn offer_draft(&mut self) {
        let Some(dir) = &self.drafts_dir else {
            return;
        };
        let found = drafts::orphans(dir, self.doc.path.as_deref());
        let files: Vec<PathBuf> = found.iter().map(|(f, _)| f.clone()).collect();
        let Some((_, draft)) = found.into_iter().next() else {
            return;
        };
        let same =
            self.doc.text == draft.text.as_str() && draft.final_newline == self.doc.final_newline;
        if same {
            for f in files {
                let _ = std::fs::remove_file(f);
            }
            return;
        }
        self.prompt = Some(Prompt::Recover(draft, files));
    }

    /// A file name typed after `:w`: `~/` is home, a relative name sits
    /// beside the current file (or in the folder Omavim started in).
    fn resolve(&self, file: &str) -> PathBuf {
        if let Some(rest) = file.strip_prefix("~/")
            && let Some(home) = std::env::var_os("HOME")
        {
            return PathBuf::from(home).join(rest);
        }
        let path = PathBuf::from(file);
        if path.is_absolute() {
            return path;
        }
        match self.doc.path.as_ref().and_then(|p| p.parent()) {
            Some(dir) => dir.join(path),
            None => path,
        }
    }

    fn save(&mut self) -> Task<Message> {
        match self.doc.path.clone() {
            Some(path) => self.write(path),
            None => self.save_as(),
        }
    }

    fn save_as(&mut self) -> Task<Message> {
        let name = if self.doc.path.is_some() {
            self.doc.name()
        } else {
            "Untitled.md".into()
        };
        let folder = self
            .doc
            .path
            .as_ref()
            .and_then(|p| p.parent())
            .map(PathBuf::from);
        Task::perform(portal::save_as(name, folder), Message::SaveTo)
    }

    /// Write the file, asking first if that's over changes made to it
    /// outside since it was read.
    fn write(&mut self, path: PathBuf) -> Task<Message> {
        if self.doc.path.as_ref() == Some(&path)
            && self.doc.disk.is_some()
            && document::disk_stamp(&path) != self.doc.disk
            && std::fs::read_to_string(&path).is_ok_and(|c| c != self.doc.contents())
        {
            self.prompt = Some(Prompt::Overwrite(path));
            return Task::none();
        }
        self.write_now(path)
    }

    fn write_now(&mut self, path: PathBuf) -> Task<Message> {
        self.writing = true;
        Task::perform(portal::write(path, self.doc.contents()), Message::Saved)
    }

    /// Has the file changed on disk? Load it if there's nothing here to
    /// lose, else ask (as Vim does when it gets focus back).
    fn check_disk(&mut self) -> Task<Message> {
        if self.prompt.is_some() || self.writing {
            return Task::none();
        }
        let Some(path) = self.doc.path.clone() else {
            return Task::none();
        };
        let now = document::disk_stamp(&path);
        if now == self.doc.disk {
            return Task::none();
        }
        let before = std::mem::replace(&mut self.doc.disk, now);
        if now.is_none() {
            if before.is_some() {
                self.status = Some(format!(
                    "E211: File \"{}\" no longer available",
                    self.doc.name()
                ));
            }
            return Task::none();
        }
        let Ok(contents) = std::fs::read_to_string(&path) else {
            return Task::none();
        };
        if contents == self.doc.contents() {
            return Task::none();
        }
        if self.doc.dirty {
            self.prompt = Some(Prompt::Changed(contents));
            return Task::none();
        }
        self.status = Some(format!(
            "\"{}\" changed on disk: loaded again",
            self.doc.name()
        ));
        self.load(&contents)
    }

    /// The file's text from disk in place of this, as one undo step.
    fn load(&mut self, contents: &str) -> Task<Message> {
        let (body, final_newline) = document::split_final_newline(contents);
        let mut text = Recorder::new(&mut self.doc);
        self.vim.reload(&mut text, body);
        self.doc.final_newline = final_newline;
        self.doc.dirty = false;
        // (Nothing unsaved now: the draft goes.)
        self.draft_due = Some(std::time::Instant::now());
        self.parse_later()
    }

    fn open(&mut self) -> Task<Message> {
        self.leave(Then::Open)
    }

    /// Leave this document (to quit, or open another): at once, or after
    /// asking about unsaved changes.
    fn leave(&mut self, then: Then) -> Task<Message> {
        if self.doc.dirty {
            self.prompt = Some(Prompt::Save(then));
            return Task::none();
        }
        self.go(then)
    }

    fn go(&mut self, then: Then) -> Task<Message> {
        match then {
            Then::Quit => self.quit(),
            Then::Open => Task::perform(portal::open(), Message::Opened),
            Then::Edit(file) => {
                self.edit(file.as_deref(), true);
                Task::none()
            }
        }
    }

    /// The answer to a prompt (Enter is the first choice, as in Vim).
    fn prompt_key(&mut self, keys: &[Key]) -> Task<Message> {
        let then = match self.prompt.clone() {
            Some(Prompt::Save(then)) => then,
            Some(Prompt::Changed(contents)) => {
                return match keys {
                    [Key::Char('l' | 'L')] => {
                        self.prompt = None;
                        self.load(&contents)
                    }
                    [Key::Char('o' | 'O') | Key::Enter | Key::Esc] => {
                        self.prompt = None;
                        Task::none()
                    }
                    _ => Task::none(),
                };
            }
            Some(Prompt::Overwrite(path)) => {
                return match keys {
                    [Key::Char('y' | 'Y')] => {
                        self.prompt = None;
                        self.write_now(path)
                    }
                    [Key::Char('n' | 'N') | Key::Enter | Key::Esc] => {
                        self.prompt = None;
                        self.after_save = None;
                        Task::none()
                    }
                    _ => Task::none(),
                };
            }
            Some(Prompt::Recover(draft, files)) => {
                let task = match keys {
                    [Key::Char('r' | 'R') | Key::Enter] => {
                        let mut text = Recorder::new(&mut self.doc);
                        self.vim.reload(&mut text, &draft.text);
                        self.doc.final_newline = draft.final_newline;
                        self.doc.dirty = true;
                        self.draft_due = Some(std::time::Instant::now());
                        self.parse_later()
                    }
                    [Key::Char('d' | 'D')] => Task::none(),
                    _ => return Task::none(),
                };
                self.prompt = None;
                for f in files {
                    let _ = std::fs::remove_file(f);
                }
                return task;
            }
            None => return Task::none(),
        };
        match keys {
            [Key::Char('y' | 'Y') | Key::Enter] => {
                self.prompt = None;
                self.after_save = Some(then);
                self.save()
            }
            [Key::Char('n' | 'N')] => {
                self.prompt = None;
                self.go(then)
            }
            [Key::Char('c' | 'C') | Key::Esc | Key::Ctrl('c')] => {
                self.prompt = None;
                Task::none()
            }
            _ => Task::none(),
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let palette = self.colors.palette();
        let dim = Color {
            a: 0.45,
            ..palette.text
        };
        let mode = self.vim.mode();
        let block = self.vim.visual_block(&self.doc.text);
        let selection = self
            .vim
            .visual_start()
            .filter(|_| block.is_none())
            .map(|start| (start, self.vim.cursor(), mode == Mode::VisualLine));
        let (line, col) = omavim_vim::text::line_col(
            &self.doc.text,
            self.vim.cursor().min(self.doc.text.len_chars()),
        );
        // While a `:` command or a search is typed, it takes the status's
        // place (and the keys typed for it aren't shown again as pending).
        let command_line = self.vim.command_line_cursor();
        let pending: String = match command_line {
            Some(_) => String::new(),
            None if self.leader_pending => self.config.leader_name(),
            None => self.vim.pending().iter().map(key_label).collect(),
        };
        let status = match command_line {
            // The cursor: a block at the end, a bar between chars.
            Some((line, at)) => {
                let before: String = line.chars().take(at).collect();
                let after: String = line.chars().skip(at).collect();
                if after.is_empty() {
                    format!("{before}█")
                } else {
                    format!("{before}▏{after}")
                }
            }
            // (Vim's words for each.)
            None => match &self.prompt {
                Some(Prompt::Save(_)) => format!(
                    "Save changes to \"{}\"?  [Y]es, (N)o, (C)ancel",
                    self.doc.name()
                ),
                Some(Prompt::Changed(_)) => format!(
                    "W12: Warning: File \"{}\" has changed and the buffer was changed \
                     in Omavim as well  [O]K, (L)oad File",
                    self.doc.name()
                ),
                Some(Prompt::Recover(draft, _)) => format!(
                    "Found unsaved changes to \"{}\" from {}, left when Omavim closed \
                     without saving them:  [R]ecover, (D)elete",
                    self.doc.name(),
                    ago(draft.written)
                ),
                Some(Prompt::Overwrite(_)) => "WARNING: The file has been changed since \
                                               reading it!!! Do you really want to write \
                                               to it (y/n)?"
                    .into(),
                None => self.status.clone().unwrap_or_default(),
            },
        };
        // Matches near the cursor: the editor follows the cursor, so the
        // lines on screen are among these.
        let near = line.saturating_sub(200)..line + 200;
        let footer = row![
            text(mode_label(mode))
                .size(SMALL_SIZE * self.scale)
                .color(if mode == Mode::Normal {
                    dim
                } else {
                    palette.primary
                }),
            // As Neovim shows it while `q{reg}` records.
            text(
                self.vim
                    .macro_register()
                    .map(|r| format!("  recording @{r}"))
                    .unwrap_or_default()
            )
            .size(SMALL_SIZE * self.scale)
            .color(palette.primary),
            text(format!(
                "  {}{}",
                self.doc.name(),
                if self.doc.dirty { " •" } else { "" }
            ))
            .size(SMALL_SIZE * self.scale)
            .color(dim),
            space::horizontal(),
            text(status)
                .size(SMALL_SIZE * self.scale)
                .color(if self.prompt.is_some() {
                    palette.primary
                } else {
                    dim
                }),
            space::horizontal(),
            text(pending).size(SMALL_SIZE * self.scale).color(dim),
            text(format!("   {}:{}", line + 1, col + 1))
                .size(SMALL_SIZE * self.scale)
                .color(dim),
        ]
        .padding([8, 16]);
        let view = View {
            text: &self.doc.text,
            cursor: self.vim.cursor(),
            mode,
            selection,
            block,
            matches: self.vim.search_highlights(&self.doc.text, near),
            current_match: self.vim.search_preview(&self.doc.text),
            tabstop: self.vim.tabstop,
            top: self.vim.top(),
            highlights: self.highlights(),
        };
        let editor = Editor::new(
            view,
            FONT,
            TEXT_SIZE * self.scale,
            Message::Key,
            Message::Resized,
            Message::Scroll,
        );
        let main: Element<'_, Message> = if self.help {
            // Over the editor, which keeps the keys (Esc or q closes it).
            iced::widget::stack![editor, self.help_view()].into()
        } else {
            editor.into()
        };
        column![
            container(main).width(Length::Fill).height(Length::Fill),
            footer,
        ]
        .into()
    }

    /// The key reference.
    fn help_view(&self) -> Element<'_, Message> {
        let palette = self.colors.palette();
        let dim = Color {
            a: 0.6,
            ..palette.text
        };
        let mut body = column![
            text("Keys")
                .size(TEXT_SIZE * 1.4 * self.scale)
                .color(palette.primary),
            text("Esc or q to close")
                .size(SMALL_SIZE * self.scale)
                .color(dim),
        ]
        .spacing(6);
        for (title, rows) in help::sections(&self.config) {
            body = body.push(space::vertical().height(12));
            body = body.push(
                text(title)
                    .size(TEXT_SIZE * self.scale)
                    .color(palette.primary),
            );
            for (keys, does) in rows {
                body = body.push(
                    row![
                        text(keys)
                            .size(14.0 * self.scale)
                            .width(Length::Fixed(260.0 * self.scale)),
                        text(does).size(14.0 * self.scale).color(dim),
                    ]
                    .spacing(16),
                );
            }
        }
        let panel = container(
            iced::widget::scrollable(container(body).padding([24, 32]).max_width(900))
                .id(help::ID)
                .height(Length::Fill),
        )
        .style(move |_| container::Style {
            background: Some(palette.background.into()),
            ..container::Style::default()
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill);
        panel.into()
    }
}

/// How long ago a time was, roughly.
fn ago(then: u64) -> String {
    let s = drafts::now().saturating_sub(then);
    let (n, unit) = match s {
        0..60 => return "just now".into(),
        60..3600 => (s / 60, "minute"),
        3600..86400 => (s / 3600, "hour"),
        _ => (s / 86400, "day"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

/// Fullscreen, or back to a window.
fn toggle_fullscreen() -> Task<Message> {
    use iced::window;
    window::latest().then(|id| {
        let Some(id) = id else {
            return Task::none();
        };
        window::mode(id).then(move |mode| {
            window::set_mode(
                id,
                if mode == window::Mode::Fullscreen {
                    window::Mode::Windowed
                } else {
                    window::Mode::Fullscreen
                },
            )
        })
    })
}

/// Read the clipboard and primary selection into `"+` and `"*`.
fn read_clipboards() -> Task<Message> {
    Task::batch([
        iced::clipboard::read().map(|t| Message::Clipboard('+', t)),
        iced::clipboard::read_primary().map(|t| Message::Clipboard('*', t)),
    ])
}

/// An iced key press as Vim keys: named keys, Ctrl+letter, or typed text.
fn vim_keys(press: &KeyPress) -> Vec<Key> {
    let m = press.modifiers;
    let named = match press.key.as_ref() {
        IcedKey::Named(Named::Escape) => Some(Key::Esc),
        IcedKey::Named(Named::Enter) => Some(Key::Enter),
        IcedKey::Named(Named::Backspace) => Some(Key::Backspace),
        IcedKey::Named(Named::Delete) => Some(Key::Delete),
        IcedKey::Named(Named::Tab) => Some(Key::Tab),
        IcedKey::Named(Named::ArrowUp) => Some(Key::Up),
        IcedKey::Named(Named::ArrowDown) => Some(Key::Down),
        IcedKey::Named(Named::ArrowLeft) => Some(Key::Left),
        IcedKey::Named(Named::ArrowRight) => Some(Key::Right),
        IcedKey::Named(Named::Home) => Some(Key::Home),
        IcedKey::Named(Named::End) => Some(Key::End),
        IcedKey::Named(Named::PageUp) => Some(Key::PageUp),
        IcedKey::Named(Named::PageDown) => Some(Key::PageDown),
        _ => None,
    };
    if let Some(k) = named {
        return vec![k];
    }
    if m.control()
        && let IcedKey::Character(c) = press.key.as_ref()
        && let Some(ch) = c.chars().next()
    {
        // Ctrl-[ is Escape, as in a terminal.
        return vec![if ch == '[' {
            Key::Esc
        } else {
            Key::Ctrl(ch.to_ascii_lowercase())
        }];
    }
    if m.alt() || m.logo() {
        return Vec::new();
    }
    press
        .text
        .iter()
        .flat_map(|t| t.chars())
        .filter(|c| !c.is_control())
        .map(Key::Char)
        .collect()
}

fn mode_label(mode: Mode) -> &'static str {
    match mode {
        Mode::Normal => "NORMAL",
        Mode::Insert => "INSERT",
        Mode::Replace => "REPLACE",
        Mode::Visual => "VISUAL",
        Mode::VisualLine => "V-LINE",
        Mode::VisualBlock => "V-BLOCK",
        Mode::OperatorPending => "NORMAL",
        Mode::CommandLine => "COMMAND",
        Mode::Confirm => "CONFIRM",
    }
}

fn key_label(k: &Key) -> String {
    match k {
        Key::Char(c) => c.to_string(),
        Key::Ctrl(c) => format!("^{}", c.to_ascii_uppercase()),
        other => format!("<{other:?}>"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::keyboard::Modifiers;

    fn press(key: IcedKey, modifiers: Modifiers, text: Option<&str>) -> KeyPress {
        KeyPress {
            key,
            modifiers,
            text: text.map(str::to_string),
        }
    }

    fn app(doc: Document) -> App {
        App {
            doc,
            vim: Vim::new(),
            scheme: Scheme::default(),
            scale: 1.0,
            colors: Colors::builtin(Scheme::default()),
            status: None,
            after_save: None,
            prompt: None,
            writing: false,
            drafts_dir: None,
            draft_due: None,
            drafted: None,
            parsing: false,
            config: Config::default(),
            leader_pending: false,
            help: false,
            highlight_cache: Default::default(),
        }
    }

    #[test]
    fn quitting_with_unsaved_changes_is_refused() {
        let mut a = app(Document::open("/tmp/notes.md".into(), "x\n"));
        a.doc.dirty = true;
        let _ = a.command("q");
        assert!(a.status.as_deref().is_some_and(|s| s.starts_with("E37")));
        let _ = a.command("nonsense");
        assert!(a.status.as_deref().is_some_and(|s| s.starts_with("E492")));
    }

    #[test]
    fn a_written_name_sits_beside_the_current_file() {
        let a = app(Document::open("/home/me/notes/a.md".into(), ""));
        assert_eq!(a.resolve("b.md"), PathBuf::from("/home/me/notes/b.md"));
        assert_eq!(a.resolve("/tmp/c.md"), PathBuf::from("/tmp/c.md"));
    }

    /// Type keys as text (Escape as `<Esc>`, Enter as `<CR>`).
    fn typed(a: &mut App, keys: &str) {
        let mut rest = keys;
        while let Some(c) = rest.chars().next() {
            let (key, text, len) = if let Some(r) = rest.strip_prefix("<Esc>") {
                (IcedKey::Named(Named::Escape), None, rest.len() - r.len())
            } else if let Some(r) = rest.strip_prefix("<CR>") {
                (IcedKey::Named(Named::Enter), None, rest.len() - r.len())
            } else {
                let s = c.to_string();
                (IcedKey::Character(s.as_str().into()), Some(s), c.len_utf8())
            };
            let _ = a.key(press(key, Modifiers::empty(), text.as_deref()));
            rest = &rest[len..];
        }
    }

    #[test]
    fn syntax_text_objects_follow_the_edits() {
        let mut a = app(Document::open(
            "/tmp/a.rs".into(),
            "fn a() {\n    one();\n}\n\nfn b(x: u8) {\n    two();\n}\n",
        ));
        // An edit, then a text object that needs the tree to know of it.
        typed(&mut a, "Ofn z() {}<Esc>");
        typed(&mut a, "G]f[fdaf");
        assert_eq!(
            a.doc.text.to_string(),
            "fn z() {}\nfn a() {\n    one();\n}\n"
        );
        // `fn z` has nothing inside: the next function's.
        typed(&mut a, "ggcifzero<Esc>");
        assert_eq!(a.doc.text.to_string(), "fn z() {}\nfn a() {\n    zero\n}\n");
    }

    #[test]
    fn leader_keys() {
        let mut a = app(Document::open("/tmp/a.md".into(), "some words"));
        typed(&mut a, "w b");
        assert_eq!(a.doc.text.to_string(), "some **words**");
        assert!(a.doc.dirty);
        typed(&mut a, "u");
        assert_eq!(a.doc.text.to_string(), "some words");
        // In visual mode, around the selection.
        typed(&mut a, "0ve i");
        assert_eq!(a.doc.text.to_string(), "_some_ words");
        assert_eq!(a.vim.mode(), Mode::Normal);
        typed(&mut a, " ?");
        assert!(a.help);
        typed(&mut a, "x");
        assert!(a.help, "other keys don't close it");
        typed(&mut a, "q");
        assert!(!a.help);
        assert_eq!(a.doc.text.to_string(), "_some_ words");
    }

    #[test]
    fn space_is_vims_after_a_count_or_an_operator() {
        let mut a = app(Document::open("/tmp/a.md".into(), "abcdef"));
        typed(&mut a, "2 ");
        assert_eq!(a.vim.cursor(), 2);
        typed(&mut a, "d ");
        assert_eq!(a.doc.text.to_string(), "abdef");
        // A leader then a key that isn't one: nothing, and the key's gone.
        typed(&mut a, " zx");
        assert_eq!(a.doc.text.to_string(), "abef");
        assert!(!a.leader_pending);
        // In insert mode it's a space.
        typed(&mut a, "i <Esc>");
        assert_eq!(a.doc.text.to_string(), "ab ef");
    }

    #[test]
    fn the_config_moves_the_leader() {
        let mut a = app(Document::open("/tmp/a.md".into(), "word"));
        a.config = Config::parse("leader = \",\"\n[keys]\nbold = \"s\"").unwrap();
        typed(&mut a, ",s");
        assert_eq!(a.doc.text.to_string(), "**word**");
    }

    #[test]
    fn unsaved_changes_are_asked_about() {
        let dir = std::env::temp_dir().join(format!("omavim-confirm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a_md, b_md) = (dir.join("a.md"), dir.join("b.md"));
        std::fs::write(&b_md, "bee\n").unwrap();
        let mut a = app(Document::open(a_md.clone(), "x"));
        // Nothing to ask about: `:confirm e` just opens.
        typed(&mut a, &format!(":confirm e {}<CR>", b_md.display()));
        assert_eq!(a.doc.text.to_string(), "bee");
        typed(&mut a, "x");
        typed(&mut a, " q");
        assert_eq!(a.prompt, Some(Prompt::Save(Then::Quit)));
        typed(&mut a, "z");
        assert_eq!(
            a.prompt,
            Some(Prompt::Save(Then::Quit)),
            "only an answer ends it"
        );
        typed(&mut a, "c");
        assert_eq!(a.prompt, None);
        assert_eq!(a.doc.text.to_string(), "ee", "cancel changes nothing");
        // No: the other file, dropping the change.
        typed(&mut a, &format!(":confirm e {}<CR>", a_md.display()));
        assert_eq!(
            a.prompt,
            Some(Prompt::Save(Then::Edit(Some(a_md.display().to_string()))))
        );
        typed(&mut a, "n");
        assert_eq!(a.doc.path.as_deref(), Some(a_md.as_path()));
        assert!(!a.doc.dirty);
        // Yes: saves (the file is written in a task), then goes on.
        typed(&mut a, "iy<Esc> q");
        typed(&mut a, "y");
        assert_eq!(a.prompt, None);
        assert_eq!(a.after_save, Some(Then::Quit));
        // Vim's own :q still refuses.
        typed(&mut a, ":q<CR>");
        assert!(a.status.as_deref().is_some_and(|s| s.starts_with("E37")));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A file of its own, in a folder of its own.
    fn scratch(name: &str, contents: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("omavim-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("notes.md");
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn an_outside_change_loads_when_nothing_here_is_lost() {
        let path = scratch("reload", "one\ntwo\n");
        let mut a = app(Document::open(path.clone(), "one\ntwo\n"));
        typed(&mut a, "j");
        let _ = a.check_disk();
        assert_eq!(a.doc.text.to_string(), "one\ntwo", "nothing changed yet");
        std::fs::write(&path, "one\ntwo\nthree\n").unwrap();
        let _ = a.check_disk();
        assert_eq!(a.doc.text.to_string(), "one\ntwo\nthree");
        assert!(!a.doc.dirty);
        assert_eq!(a.vim.cursor(), 4, "still on two");
        // And `u` has the old text back.
        typed(&mut a, "u");
        assert_eq!(a.doc.text.to_string(), "one\ntwo");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn an_outside_change_over_changes_here_asks() {
        let path = scratch("changed", "one\n");
        let mut a = app(Document::open(path.clone(), "one\n"));
        typed(&mut a, "Aa<Esc>");
        std::fs::write(&path, "one two\n").unwrap();
        let _ = a.check_disk();
        assert_eq!(a.prompt, Some(Prompt::Changed("one two\n".into())));
        typed(&mut a, "o");
        assert_eq!(a.doc.text.to_string(), "onea", "OK keeps this");
        let _ = a.check_disk();
        assert_eq!(a.prompt, None, "and doesn't ask again for the same change");
        std::fs::write(&path, "one two three\n").unwrap();
        let _ = a.check_disk();
        typed(&mut a, "l");
        assert_eq!(a.doc.text.to_string(), "one two three");
        assert!(!a.doc.dirty);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn saving_over_an_outside_change_asks() {
        let path = scratch("overwrite", "one\n");
        let mut a = app(Document::open(path.clone(), "one\n"));
        typed(&mut a, "Aa<Esc>");
        std::fs::write(&path, "theirs\n").unwrap();
        typed(&mut a, ":w<CR>");
        assert_eq!(a.prompt, Some(Prompt::Overwrite(path.clone())));
        typed(&mut a, "n");
        assert_eq!(a.prompt, None);
        assert!(!a.writing, "nothing written");
        typed(&mut a, ":w<CR>y");
        assert!(a.writing);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_file_gone_from_disk_says_so() {
        let path = scratch("gone", "one\n");
        let mut a = app(Document::open(path.clone(), "one\n"));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
        let _ = a.check_disk();
        assert!(a.status.as_deref().is_some_and(|s| s.starts_with("E211")));
        assert_eq!(a.doc.text.to_string(), "one");
    }

    #[test]
    fn a_draft_is_written_and_goes_on_save() {
        let path = scratch("draft", "one\n");
        let dir = path.parent().unwrap().join("drafts");
        let mut a = app(Document::open(path.clone(), "one\n"));
        a.drafts_dir = Some(dir.clone());
        typed(&mut a, "Atwo<Esc>");
        assert!(a.draft_due.is_some());
        let _ = a.update(Message::Unfocused);
        let mine: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
        assert_eq!(mine.len(), 1);
        let draft: drafts::Draft =
            toml::from_str(&std::fs::read_to_string(mine[0].path()).unwrap()).unwrap();
        assert_eq!(draft.text, "onetwo");
        assert_eq!(draft.pid, std::process::id());
        let _ = a.update(Message::Saved(Ok(path.clone())));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_draft_left_behind_is_offered_back() {
        let path = scratch("recover", "one\n");
        let dir = path.parent().unwrap().join("drafts");
        let left = |text: &str| drafts::Draft {
            path: Some(path.clone()),
            // (Past the kernel's limit: never a running process.)
            pid: 4_000_001,
            written: drafts::now() - 300,
            final_newline: true,
            text: text.into(),
        };
        drafts::write(&dir, &left("one\nmore")).unwrap();
        let mut a = app(Document::open(path.clone(), "one\n"));
        a.drafts_dir = Some(dir.clone());
        a.offer_draft();
        assert!(matches!(a.prompt, Some(Prompt::Recover(..))));
        typed(&mut a, "x");
        assert!(a.prompt.is_some(), "only an answer ends it");
        typed(&mut a, "r");
        assert_eq!(a.doc.text.to_string(), "one\nmore");
        assert!(a.doc.dirty);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "taken back");
        // `u` is the file as it was.
        typed(&mut a, "u");
        assert_eq!(a.doc.text.to_string(), "one");
        // One that's the same as the file just goes.
        drafts::write(&dir, &left("one")).unwrap();
        let mut a = app(Document::open(path.clone(), "one\n"));
        a.drafts_dir = Some(dir.clone());
        a.offer_draft();
        assert_eq!(a.prompt, None);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn follows_the_desktops_text_size() {
        let mut a = app(Document::default());
        let _ = a.update(Message::TextScale(1.3636));
        assert_eq!(a.scale, 1.3636);
        let _ = a.update(Message::TextScale(40.0));
        assert_eq!(a.scale, 3.0, "kept readable");
    }

    #[test]
    fn set_filetype_changes_the_language() {
        let mut a = app(Document::open("/tmp/notes.txt".into(), "fn a() {}\n"));
        assert!(a.doc.syntax.is_none());
        typed(&mut a, ":set ft=rust<CR>");
        assert_eq!(a.doc.filetype(), "rust");
        typed(&mut a, "dif");
        assert_eq!(a.doc.text.to_string(), "fn a() {}");
        typed(&mut a, ":set ft?<CR>");
        assert_eq!(a.status.as_deref(), Some("  filetype=rust"));
        typed(&mut a, ":set ft=nonesuch<CR>");
        assert!(a.doc.syntax.is_none());
    }

    #[test]
    fn turns_key_presses_into_vim_keys() {
        assert_eq!(
            vim_keys(&press(
                IcedKey::Named(Named::Escape),
                Modifiers::empty(),
                None
            )),
            [Key::Esc]
        );
        assert_eq!(
            vim_keys(&press(
                IcedKey::Character("r".into()),
                Modifiers::CTRL,
                None
            )),
            [Key::Ctrl('r')]
        );
        assert_eq!(
            vim_keys(&press(
                IcedKey::Character("[".into()),
                Modifiers::CTRL,
                None
            )),
            [Key::Esc]
        );
        assert_eq!(
            vim_keys(&press(
                IcedKey::Character("é".into()),
                Modifiers::empty(),
                Some("é")
            )),
            [Key::Char('é')]
        );
        assert!(
            vim_keys(&press(
                IcedKey::Character("x".into()),
                Modifiers::LOGO,
                Some("x")
            ))
            .is_empty()
        );
    }
}
