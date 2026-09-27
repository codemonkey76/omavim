//! The editor: Omavim's own iced widget, because iced's `text_editor` can't
//! give Vim what it needs (a cursor per mode, visual selections, every key).
//!
//! It draws the document's visible lines in a centred column and the cursor,
//! scrolls to keep the cursor in view, and hands every key press to the app.
//! Milestone 1: no wrapping and no highlighting yet.

use crate::document::Document;
use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self, Quad, Renderer as _};
use iced::advanced::text::{self, Paragraph as _, Renderer as _};
use iced::advanced::widget::{self, Widget, tree};
use iced::advanced::{Clipboard, Shell};
use iced::keyboard;
use iced::mouse;
use iced::{Element, Event, Font, Length, Pixels, Point, Rectangle, Size, Theme};
use std::cell::Cell;

/// Characters in the text column (Omawrite's measure, roughly).
const COLUMN: f32 = 72.0;
/// Line height, relative to the text size.
const LINE_HEIGHT: f32 = 1.6;
/// Space above the first line and below the last.
const PADDING: f32 = 48.0;
/// Spaces a tab is drawn as.
const TAB: usize = 4;

/// A key press, as the app needs it.
#[derive(Debug, Clone)]
pub struct KeyPress {
    pub key: keyboard::Key,
    pub modifiers: keyboard::Modifiers,
    /// What the key types, if anything.
    pub text: Option<String>,
}

pub struct Editor<'a, Message> {
    doc: &'a Document,
    font: Font,
    size: f32,
    on_key: Box<dyn Fn(KeyPress) -> Message + 'a>,
}

impl<'a, Message> Editor<'a, Message> {
    pub fn new(
        doc: &'a Document,
        font: Font,
        size: f32,
        on_key: impl Fn(KeyPress) -> Message + 'a,
    ) -> Self {
        Self {
            doc,
            font,
            size,
            on_key: Box::new(on_key),
        }
    }
}

#[derive(Default)]
struct State {
    /// The first line on screen.
    top: Cell<usize>,
    /// Where the cursor was last drawn: when it moves, the view follows it;
    /// when only the wheel scrolls, the view stays where it was put.
    last_cursor: Cell<Option<usize>>,
    /// The advance of one character at the current size.
    char_width: Cell<Option<(f32, f32)>>,
    /// Lines on screen at the last draw, for scrolling by a page.
    visible: Cell<usize>,
}

/// A line as drawn: tabs expanded, no line break.
fn display(line: &str) -> String {
    line.trim_end_matches(['\n', '\r'])
        .replace('\t', &" ".repeat(TAB))
}

/// The drawn column of the char at `col` in `line`, with tabs expanded.
fn display_col(line: &str, col: usize) -> usize {
    line.chars()
        .take(col)
        .map(|c| if c == '\t' { TAB } else { 1 })
        .sum()
}

impl<Message> Editor<'_, Message> {
    fn char_width(&self, state: &State) -> f32 {
        if let Some((size, width)) = state.char_width.get()
            && size == self.size
        {
            return width;
        }
        let probe =
            <iced::Renderer as text::Renderer>::Paragraph::with_text(iced::advanced::Text {
                content: "0000000000",
                bounds: Size::INFINITE,
                size: Pixels(self.size),
                line_height: text::LineHeight::Relative(LINE_HEIGHT),
                font: self.font,
                align_x: text::Alignment::Left,
                align_y: iced::alignment::Vertical::Top,
                shaping: text::Shaping::Advanced,
                wrapping: text::Wrapping::None,
            });
        let width = probe.min_width() / 10.0;
        state.char_width.set(Some((self.size, width)));
        width
    }
}

impl<Message> Widget<Message, Theme, iced::Renderer> for Editor<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn layout(
        &mut self,
        _tree: &mut widget::Tree,
        _renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(limits.max())
    }

    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &Event,
        _layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<State>();
        match event {
            Event::Keyboard(keyboard::Event::KeyPressed {
                key,
                modifiers,
                text,
                ..
            }) => {
                shell.publish((self.on_key)(KeyPress {
                    key: key.clone(),
                    modifiers: *modifiers,
                    text: text.as_ref().map(|t| t.to_string()),
                }));
                shell.capture_event();
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let lines = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => -y * 3.0,
                    mouse::ScrollDelta::Pixels { y, .. } => -y / (self.size * LINE_HEIGHT),
                };
                let last = self.doc.text.len_lines().saturating_sub(1) as f32;
                let top = (state.top.get() as f32 + lines).clamp(0.0, last);
                state.top.set(top.round() as usize);
                shell.request_redraw();
                shell.capture_event();
            }
            _ => {}
        }
    }

    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut iced::Renderer,
        theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<State>();
        let palette = theme.palette();
        let bounds = layout.bounds();
        let char_w = self.char_width(state);
        let line_h = self.size * LINE_HEIGHT;

        let column_w = (char_w * COLUMN)
            .min(bounds.width - 2.0 * PADDING)
            .max(char_w * 10.0);
        let left = bounds.x + ((bounds.width - column_w) / 2.0).max(0.0);
        let visible = (((bounds.height - 2.0 * PADDING) / line_h).floor() as usize).max(1);
        state.visible.set(visible);

        // Follow the cursor when it has moved.
        let (cursor_line, cursor_col) = self.doc.line_col();
        if state.last_cursor.get() != Some(self.doc.cursor) {
            let top = state.top.get();
            if cursor_line < top {
                state.top.set(cursor_line);
            } else if cursor_line >= top + visible {
                state.top.set(cursor_line + 1 - visible);
            }
            state.last_cursor.set(Some(self.doc.cursor));
        }
        let top = state.top.get();

        let lines = self.doc.text.len_lines();
        for (row, line) in (top..lines.min(top + visible)).enumerate() {
            let y = bounds.y + PADDING + row as f32 * line_h;
            let content = display(&self.doc.text.line(line).to_string());
            if content.is_empty() {
                continue;
            }
            renderer.fill_text(
                iced::advanced::Text {
                    content,
                    bounds: Size::new(f32::INFINITY, line_h),
                    size: Pixels(self.size),
                    line_height: text::LineHeight::Relative(LINE_HEIGHT),
                    font: self.font,
                    align_x: text::Alignment::Left,
                    align_y: iced::alignment::Vertical::Top,
                    shaping: text::Shaping::Advanced,
                    wrapping: text::Wrapping::None,
                },
                Point::new(left, y),
                palette.text,
                bounds,
            );
        }

        // The cursor: a bar for now; per-mode shapes come with Vim.
        if (top..top + visible).contains(&cursor_line) {
            let line = self.doc.text.line(cursor_line).to_string();
            let x = left + display_col(&line, cursor_col) as f32 * char_w;
            let y = bounds.y + PADDING + (cursor_line - top) as f32 * line_h;
            renderer.fill_quad(
                Quad {
                    bounds: Rectangle::new(
                        Point::new(x, y + line_h * 0.1),
                        Size::new(2.0, line_h * 0.8),
                    ),
                    ..Quad::default()
                },
                palette.primary,
            );
        }
    }

    fn mouse_interaction(
        &self,
        _tree: &widget::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        if cursor.is_over(layout.bounds()) {
            mouse::Interaction::Text
        } else {
            mouse::Interaction::default()
        }
    }
}

impl<'a, Message: 'a> From<Editor<'a, Message>> for Element<'a, Message> {
    fn from(editor: Editor<'a, Message>) -> Self {
        Element::new(editor)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn tabs_are_drawn_as_spaces_and_counted_so() {
        assert_eq!(super::display("\tx\n"), "    x");
        assert_eq!(super::display_col("\tab", 2), 5);
        assert_eq!(super::display_col("abc", 2), 2);
    }
}
