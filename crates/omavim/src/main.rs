//! Omavim: a dead-simple writing app with Vim motions. See PLAN.md.

mod document;
mod editor;
mod portal;

use document::Document;
use editor::{Editor, KeyPress};
use iced::keyboard::{Key, key::Named};
use iced::widget::{column, container, row, space, text};
use iced::{Color, Element, Font, Length, Subscription, Task, Theme};
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

    /// Milestone 1's keys: plain typing, and Ctrl+S / Ctrl+Shift+S / Ctrl+O.
    /// Vim takes over in milestone 2.
    fn key(&mut self, press: KeyPress) -> Task<Message> {
        let m = press.modifiers;
        if m.control() {
            return match press.key.as_ref() {
                Key::Character(c) if c.eq_ignore_ascii_case("s") && m.shift() => self.save_as(),
                Key::Character(c) if c.eq_ignore_ascii_case("s") => self.save(),
                Key::Character(c) if c.eq_ignore_ascii_case("o") => self.open(),
                _ => Task::none(),
            };
        }
        self.status = None;
        match press.key.as_ref() {
            Key::Named(Named::Enter) => self.doc.insert("\n"),
            Key::Named(Named::Tab) => self.doc.insert("\t"),
            Key::Named(Named::Backspace) => self.doc.backspace(),
            Key::Named(Named::Delete) => self.doc.delete(),
            Key::Named(Named::ArrowLeft) => self.doc.left(),
            Key::Named(Named::ArrowRight) => self.doc.right(),
            Key::Named(Named::ArrowUp) => self.doc.vertical(-1),
            Key::Named(Named::ArrowDown) => self.doc.vertical(1),
            Key::Named(Named::PageUp) => self.doc.vertical(-20),
            Key::Named(Named::PageDown) => self.doc.vertical(20),
            Key::Named(Named::Home) => self.doc.home(),
            Key::Named(Named::End) => self.doc.end(),
            _ => {
                if let Some(t) = press
                    .text
                    .filter(|t| !m.alt() && !m.logo() && t.chars().all(|c| !c.is_control()))
                {
                    self.doc.insert(&t);
                }
            }
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
        Task::perform(
            portal::write(path, self.doc.text.to_string()),
            Message::Saved,
        )
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
        let dim = {
            let mut c = theme(self.scheme).palette().text;
            c.a = 0.45;
            c
        };
        let (line, col) = self.doc.line_col();
        let footer = row![
            text(format!(
                "{}{}",
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
            text(format!("{}:{}", line + 1, col + 1))
                .size(13)
                .color(dim),
        ]
        .padding([8, 16]);
        column![
            container(Editor::new(&self.doc, FONT, TEXT_SIZE, Message::Key))
                .width(Length::Fill)
                .height(Length::Fill),
            footer,
        ]
        .into()
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
