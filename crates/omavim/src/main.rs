//! Omavim: a dead-simple writing app with Vim motions. See PLAN.md.

mod document;
mod editor;
mod portal;
mod wrap;

use document::Document;
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
    /// A short message in the footer: an error, or what just happened.
    status: Option<String>,
}

#[derive(Debug, Clone)]
enum Message {
    Key(KeyPress),
    Scheme(Scheme),
    Opened(Result<Option<(PathBuf, String)>, String>),
    /// Where Save As chose to write (None: cancelled).
    SaveTo(Result<Option<PathBuf>, String>),
    Saved(Result<PathBuf, String>),
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
        Self {
            doc,
            vim: Vim::new(),
            scheme: Scheme::default(),
            status,
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
        Some(theme(self.scheme))
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::run(portal::color_scheme).map(Message::Scheme)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Key(press) => return self.key(press),
            Message::Scheme(scheme) => self.scheme = scheme,
            Message::Opened(Ok(Some((path, contents)))) => {
                self.doc = Document::open(path, &contents);
                self.vim = Vim::new();
                self.status = None;
            }
            Message::Opened(Ok(None)) | Message::SaveTo(Ok(None)) => {}
            Message::SaveTo(Ok(Some(path))) => return self.write(path),
            Message::Saved(Ok(path)) => {
                self.status = Some(format!("Saved {}", path.display()));
                self.doc.path = Some(path);
                self.doc.dirty = false;
            }
            Message::Opened(Err(e)) | Message::SaveTo(Err(e)) | Message::Saved(Err(e)) => {
                self.status = Some(e)
            }
        }
        Task::none()
    }

    /// Keys go to Vim, except the app's own: Ctrl+S, Ctrl+Shift+S, Ctrl+O
    /// (for now: the leader keys replace them in a later milestone).
    fn key(&mut self, press: KeyPress) -> Task<Message> {
        let m = press.modifiers;
        if m.control()
            && let IcedKey::Character(c) = press.key.as_ref()
        {
            match c.to_ascii_lowercase().as_str() {
                "s" if m.shift() => return self.save_as(),
                "s" => return self.save(),
                "o" => return self.open(),
                _ => {}
            }
        }
        let before = self.vim.changes();
        for key in vim_keys(&press) {
            if self.vim.key(&mut self.doc.text, key).is_err() {
                // Vim beeps; the keys after it still count, as typed keys do.
            }
        }
        if self.vim.changes() != before {
            self.doc.dirty = true;
            self.status = None;
        }
        Task::none()
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
        let palette = theme(self.scheme).palette();
        let dim = Color {
            a: 0.45,
            ..palette.text
        };
        let mode = self.vim.mode();
        let selection = self
            .vim
            .visual_start()
            .map(|start| (start, self.vim.cursor(), mode == Mode::VisualLine));
        let (line, col) = omavim_vim::text::line_col(
            &self.doc.text,
            self.vim.cursor().min(self.doc.text.len_chars()),
        );
        let pending: String = self.vim.pending().iter().map(key_label).collect();
        let footer = row![
            text(mode_label(mode))
                .size(13)
                .color(if mode == Mode::Normal {
                    dim
                } else {
                    palette.primary
                }),
            text(format!(
                "  {}{}",
                self.doc.name(),
                if self.doc.dirty { " •" } else { "" }
            ))
            .size(13)
            .color(dim),
            space::horizontal(),
            text(self.status.clone().unwrap_or_default())
                .size(13)
                .color(dim),
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
            tabstop: self.vim.tabstop,
        };
        column![
            container(Editor::new(view, FONT, TEXT_SIZE, Message::Key))
                .width(Length::Fill)
                .height(Length::Fill),
            footer,
        ]
        .into()
    }
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
    }
}

fn key_label(k: &Key) -> String {
    match k {
        Key::Char(c) => c.to_string(),
        Key::Ctrl(c) => format!("^{}", c.to_ascii_uppercase()),
        other => format!("<{other:?}>"),
    }
}

/// Omavim's two looks, chosen by the desktop's dark/light setting.
fn theme(scheme: Scheme) -> Theme {
    let rgb = |r, g, b| Color::from_rgb8(r, g, b);
    match scheme {
        Scheme::Light => Theme::custom(
            "Omavim Light",
            iced::theme::Palette {
                background: rgb(0xfb, 0xfa, 0xf7),
                text: rgb(0x22, 0x22, 0x22),
                primary: rgb(0x1e, 0x6f, 0xd9),
                success: rgb(0x2e, 0x7d, 0x32),
                warning: rgb(0xb2, 0x6b, 0x00),
                danger: rgb(0xc6, 0x28, 0x28),
            },
        ),
        Scheme::Dark => Theme::custom(
            "Omavim Dark",
            iced::theme::Palette {
                background: rgb(0x1b, 0x1b, 0x1b),
                text: rgb(0xdd, 0xdd, 0xdd),
                primary: rgb(0x6e, 0xa8, 0xfe),
                success: rgb(0x81, 0xc7, 0x84),
                warning: rgb(0xff, 0xb7, 0x4d),
                danger: rgb(0xef, 0x9a, 0x9a),
            },
        ),
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
