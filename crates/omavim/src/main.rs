//! Omavim: a dead-simple writing app with Vim motions. See PLAN.md.

mod colors;
mod document;
mod editor;
mod portal;

use colors::Colors;
use document::{Document, Recorder};
use editor::{Editor, KeyPress, View};
use iced::keyboard::{Key as IcedKey, key::Named};
use iced::widget::{column, container, row, space, text};
use iced::{Color, Element, Font, Length, Subscription, Task, Theme};
use omavim_vim::{Key, Mode, Vim};
use portal::Scheme;
use std::path::PathBuf;

const FONT: Font = Font::with_name("iA Writer Mono S");
/// The text size Omavim is designed around (Omawrite's is 12px at its scale).
const TEXT_SIZE: f32 = 17.0;

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
        .run()
}

struct App {
    doc: Document,
    vim: Vim,
    scheme: Scheme,
    /// The colours: the Omarchy theme's, or Omavim's for `scheme`.
    colors: Colors,
    /// A short message in the footer: an error, or what just happened.
    status: Option<String>,
    /// `:wq` / `:x` waiting for its save to finish before quitting.
    quit_after_save: bool,
    /// The last highlights worked out, and what for (the text's changes
    /// count, the first line, how many), so a redraw that changed none of
    /// that doesn't work them out again.
    highlight_cache: std::cell::RefCell<HighlightCache>,
}

type Highlights = Vec<(std::ops::Range<usize>, colors::Style)>;

#[derive(Default)]
struct HighlightCache {
    /// The text's version, its language, and the lines.
    key: Option<(u64, omavim_syntax::Lang, usize, usize)>,
    highlights: std::rc::Rc<Highlights>,
}

#[derive(Debug, Clone)]
enum Message {
    Key(KeyPress),
    Scheme(Scheme),
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
    /// Time to see if the Omarchy theme changed.
    CheckTheme,
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
        let mut vim = Vim::new();
        // For writing: j and k go by screen line, as gj and gk.
        vim.display_lines = true;
        vim.set_file_name(doc.path.as_deref().and_then(|p| p.to_str()));
        vim.set_filetype(doc.filetype());
        Self {
            doc,
            vim,
            scheme: Scheme::default(),
            colors: Colors::current(Scheme::default()),
            status,
            quit_after_save: false,
            highlight_cache: Default::default(),
        }
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
        Subscription::batch([
            Subscription::run(portal::color_scheme).map(Message::Scheme),
            // (A new Omarchy theme swaps its colours file in: cheap to check.)
            iced::time::every(std::time::Duration::from_secs(2)).map(|_| Message::CheckTheme),
            iced::event::listen_with(|event, _, _| match event {
                iced::Event::Window(iced::window::Event::Focused) => Some(Message::Focused),
                _ => None,
            }),
        ])
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Key(press) => return self.key(press),
            Message::Scheme(scheme) => {
                self.scheme = scheme;
                self.set_colors(Colors::current(scheme));
            }
            Message::CheckTheme => {
                if Colors::omarchy_stamp() != self.colors.source {
                    self.set_colors(Colors::current(self.scheme));
                }
            }
            Message::Opened(Ok(Some((path, contents)))) => {
                self.doc = Document::open(path, &contents);
                *self.highlight_cache.borrow_mut() = HighlightCache::default();
                self.vim = self.vim.for_other_text();
                self.vim
                    .set_file_name(self.doc.path.as_deref().and_then(|p| p.to_str()));
                self.vim.set_filetype(self.doc.filetype());
                self.status = None;
            }
            Message::Focused => return read_clipboards(),
            Message::Clipboard(register, text) => {
                self.vim
                    .set_clipboard(register, text.as_deref().unwrap_or(""));
            }
            Message::Resized(cells, rows) => self.vim.set_screen(&self.doc.text, cells, rows),
            Message::Scroll(rows) => self.vim.scroll_view(&self.doc.text, rows),
            Message::Opened(Ok(None)) => {}
            Message::SaveTo(Ok(None)) => self.quit_after_save = false,
            Message::SaveTo(Ok(Some(path))) => return self.write(path),
            Message::Saved(Ok(path)) => {
                self.status = Some(format!("Saved {}", path.display()));
                self.vim.set_file_name(path.to_str());
                self.doc.path = Some(path);
                // (A new name may be a new language.)
                self.doc.detect_language();
                self.vim.set_filetype(self.doc.filetype());
                self.doc.dirty = false;
                if self.quit_after_save {
                    return iced::exit();
                }
            }
            Message::Opened(Err(e)) | Message::SaveTo(Err(e)) | Message::Saved(Err(e)) => {
                self.status = Some(e)
            }
        }
        Task::none()
    }

    /// Keys go to Vim, except the app's own: Ctrl+S, Ctrl+Shift+S,
    /// Ctrl+Shift+O (for now: the leader keys replace them in a later
    /// milestone). Ctrl+O is Vim's, back through the jump list.
    fn key(&mut self, press: KeyPress) -> Task<Message> {
        let m = press.modifiers;
        if m.control()
            && let IcedKey::Character(c) = press.key.as_ref()
        {
            match c.to_ascii_lowercase().as_str() {
                "s" if m.shift() => return self.save_as(),
                "s" => return self.save(),
                "o" if m.shift() => return self.open(),
                _ => {}
            }
        }
        let before = self.vim.changes();
        let mut text = Recorder::new(&mut self.doc);
        for key in vim_keys(&press) {
            if self.vim.key(&mut text, key).is_err() {
                // Vim beeps; the keys after it still count, as typed keys do.
            }
        }
        drop(text);
        // `:set filetype=`: highlight as that language (plain text for one
        // Omavim doesn't know, as Vim keeps the name).
        if let Some(filetype) = self.vim.take_filetype() {
            self.doc
                .set_language(omavim_syntax::Lang::from_name(&filetype));
        }
        if self.vim.changes() != before {
            self.doc.dirty = true;
            self.status = None;
        }
        // "search hit BOTTOM, continuing at TOP", "E486: Pattern not found".
        if let Some(message) = self.vim.take_message() {
            self.status = Some(message);
        }
        let mut tasks = Vec::new();
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
        let key = (self.vim.changes(), syntax.lang(), first, count);
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
                self.quit_after_save = true;
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
            "w" | "w!" | "write" => match arg {
                Some(file) => self.write(self.resolve(file)),
                None => self.save(),
            },
            "q" | "quit" | "clo" | "close" if self.doc.dirty => {
                self.status = Some("E37: No write since last change (add ! to override)".into());
                Task::none()
            }
            "q" | "quit" | "q!" | "quit!" | "qa" | "qa!" | "clo" | "close" => iced::exit(),
            "x" | "xit" | "exi" | "exit" if !self.doc.dirty => iced::exit(),
            "wq" | "x" | "xit" | "exi" | "exit" => {
                self.quit_after_save = true;
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
        self.doc = doc;
        *self.highlight_cache.borrow_mut() = HighlightCache::default();
        self.status = status;
        self.vim = self.vim.for_other_text();
        self.vim
            .set_file_name(self.doc.path.as_deref().and_then(|p| p.to_str()));
        self.vim.set_filetype(self.doc.filetype());
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

    fn write(&mut self, path: PathBuf) -> Task<Message> {
        Task::perform(portal::write(path, self.doc.contents()), Message::Saved)
    }

    fn open(&mut self) -> Task<Message> {
        if self.doc.dirty {
            // The proper unsaved-changes prompt comes in a later milestone.
            self.status = Some("Unsaved changes: save first (Ctrl+S)".into());
            return Task::none();
        }
        Task::perform(portal::open(), Message::Opened)
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
            None => self.status.clone().unwrap_or_default(),
        };
        // Matches near the cursor: the editor follows the cursor, so the
        // lines on screen are among these.
        let near = line.saturating_sub(200)..line + 200;
        let footer = row![
            text(mode_label(mode))
                .size(13)
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
            .size(13)
            .color(palette.primary),
            text(format!(
                "  {}{}",
                self.doc.name(),
                if self.doc.dirty { " •" } else { "" }
            ))
            .size(13)
            .color(dim),
            space::horizontal(),
            text(status).size(13).color(dim),
            space::horizontal(),
            text(pending).size(13).color(dim),
            text(format!("   {}:{}", line + 1, col + 1))
                .size(13)
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
        column![
            container(Editor::new(
                view,
                FONT,
                TEXT_SIZE,
                Message::Key,
                Message::Resized,
                Message::Scroll
            ))
            .width(Length::Fill)
            .height(Length::Fill),
            footer,
        ]
        .into()
    }
}

/// An iced key press as Vim keys: named keys, Ctrl+letter, or typed text.
/// Read the clipboard and primary selection into `"+` and `"*`.
fn read_clipboards() -> Task<Message> {
    Task::batch([
        iced::clipboard::read().map(|t| Message::Clipboard('+', t)),
        iced::clipboard::read_primary().map(|t| Message::Clipboard('*', t)),
    ])
}

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
            colors: Colors::builtin(Scheme::default()),
            status: None,
            quit_after_save: false,
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
