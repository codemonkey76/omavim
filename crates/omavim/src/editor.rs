//! The editor: Omavim's own iced widget, because iced's `text_editor` can't
//! give Vim what it needs (a cursor per mode, visual selections, every key).
//!
//! It draws a centred column of soft-wrapped lines, the cursor (a block in
//! normal and visual mode, a bar in insert, an underline in replace and while
//! an operator waits), and the visual selection; it scrolls by screen row to
//! keep the cursor in view, and hands every key press to the app.

use crate::wrap;
use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self, Quad, Renderer as _};
use iced::advanced::text::{self, Paragraph as _, Renderer as _};
use iced::advanced::widget::{self, Widget, tree};
use iced::advanced::{Clipboard, Shell};
use iced::keyboard;
use iced::mouse;
use iced::{Color, Element, Event, Font, Length, Pixels, Point, Rectangle, Size, Theme};
use omavim_vim::{Mode, Pos};
use ropey::Rope;
use std::cell::Cell;

/// Cells in the text column (Omawrite's measure, roughly).
const COLUMN: f32 = 72.0;
/// Line height, relative to the text size.
const LINE_HEIGHT: f32 = 1.6;
/// Space above the first row and below the last.
const PADDING: f32 = 48.0;

/// A key press, as the app needs it.
#[derive(Debug, Clone)]
pub struct KeyPress {
    pub key: keyboard::Key,
    pub modifiers: keyboard::Modifiers,
    /// What the key types, if anything.
    pub text: Option<String>,
}

/// What the editor shows: the text and Vim's state.
pub struct View<'a> {
    pub text: &'a Rope,
    pub cursor: Pos,
    pub mode: Mode,
    /// A visual selection: its two ends (in either order), and whether it's
    /// linewise. Inclusive; after `$` it takes the line break.
    pub selection: Option<(Pos, Pos, bool)>,
    pub tabstop: usize,
}

pub struct Editor<'a, Message> {
    view: View<'a>,
    font: Font,
    size: f32,
    on_key: Box<dyn Fn(KeyPress) -> Message + 'a>,
}

impl<'a, Message> Editor<'a, Message> {
    pub fn new(
        view: View<'a>,
        font: Font,
        size: f32,
        on_key: impl Fn(KeyPress) -> Message + 'a,
    ) -> Self {
        Self {
            view,
            font,
            size,
            on_key: Box::new(on_key),
        }
    }
}

#[derive(Default)]
struct State {
    /// The first screen row: a line, and a row within it.
    top: Cell<(usize, usize)>,
    /// Where the cursor was at the last draw: when it moves the view follows;
    /// when only the wheel scrolls, the view stays where it was put.
    last_cursor: Cell<Option<Pos>>,
    /// One cell's width at the current size.
    cell: Cell<Option<(f32, f32)>>,
    /// The column's width in cells at the last draw, for wheel scrolling.
    cells: Cell<usize>,
}

/// A line's chars (without its line break).
fn line_chars(t: &Rope, line: usize) -> Vec<char> {
    let slice = t.line(line);
    let mut chars: Vec<char> = slice.chars().collect();
    if chars.last() == Some(&'\n') {
        chars.pop();
    }
    chars
}

impl<Message> Editor<'_, Message> {
    fn cell_width(&self, state: &State) -> f32 {
        if let Some((size, width)) = state.cell.get()
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
        state.cell.set(Some((self.size, width)));
        width
    }

    fn rows(&self, line: usize, cells: usize) -> Vec<(usize, usize)> {
        wrap::rows(&line_chars(self.view.text, line), cells, self.view.tabstop)
    }

    /// Move a (line, row) position by `n` screen rows, within the text.
    fn step(&self, (mut line, mut row): (usize, usize), n: isize, cells: usize) -> (usize, usize) {
        let last = self.view.text.len_lines() - 1;
        if n >= 0 {
            for _ in 0..n {
                if row + 1 < self.rows(line, cells).len() {
                    row += 1;
                } else if line < last {
                    line += 1;
                    row = 0;
                }
            }
        } else {
            for _ in 0..-n {
                if row > 0 {
                    row -= 1;
                } else if line > 0 {
                    line -= 1;
                    row = self.rows(line, cells).len() - 1;
                }
            }
        }
        (line, row)
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
                let rows = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => -y * 3.0,
                    mouse::ScrollDelta::Pixels { y, .. } => -y / (self.size * LINE_HEIGHT),
                };
                let cells = state.cells.get().max(10);
                state
                    .top
                    .set(self.step(state.top.get(), rows.round() as isize, cells));
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
        let cell = self.cell_width(state);
        let row_h = self.size * LINE_HEIGHT;
        let t = self.view.text;
        let ts = self.view.tabstop;

        let column_w = (cell * COLUMN)
            .min(bounds.width - 2.0 * PADDING)
            .max(cell * 10.0);
        let cells = (column_w / cell).floor() as usize;
        state.cells.set(cells);
        let left = bounds.x + ((bounds.width - column_w) / 2.0).max(0.0);
        let visible = (((bounds.height - 2.0 * PADDING) / row_h).floor() as usize).max(1);

        // Follow the cursor when it has moved.
        let cursor = self.view.cursor.min(t.len_chars());
        let cline = t.char_to_line(cursor);
        let ccol = cursor - t.line_to_char(cline);
        let crow = wrap::row_of(&self.rows(cline, cells), ccol);
        if state.last_cursor.get() != Some(cursor) {
            let top = state.top.get();
            if (cline, crow) < top {
                state.top.set((cline, crow));
            } else {
                // Rows from the top to the cursor; scroll if it's past the bottom.
                let mut n = 0;
                let mut p = top;
                while p < (cline, crow) && n < visible + 1 {
                    p = self.step(p, 1, cells);
                    n += 1;
                }
                if n >= visible {
                    state
                        .top
                        .set(self.step((cline, crow), -(visible as isize - 1), cells));
                }
            }
            state.last_cursor.set(Some(cursor));
        }

        // Selection as a char range, and whether line breaks are in it.
        let selection = self.view.selection.map(|(a, b, linewise)| {
            let (a, b) = (a.min(b), a.max(b));
            if linewise {
                let (al, bl) = (t.char_to_line(a), t.char_to_line(b));
                (
                    t.line_to_char(al),
                    t.line_to_char(bl) + line_chars(t, bl).len() + 1,
                )
            } else {
                (a, b + 1)
            }
        });
        let mut selected = palette.primary;
        selected.a = 0.22;

        let (mut line, mut row) = state.top.get();
        for screen_row in 0..visible {
            if line >= t.len_lines() {
                break;
            }
            let chars = line_chars(t, line);
            let vc = wrap::vcols(&chars, ts);
            let rows = wrap::rows(&chars, cells, ts);
            let (start, end) = rows[row.min(rows.len() - 1)];
            let y = bounds.y + PADDING + screen_row as f32 * row_h;
            let x_of = |col: usize| left + (vc[col.min(chars.len())] - vc[start]) as f32 * cell;
            let line_start = t.line_to_char(line);

            // The selection on this row, and the line break as one cell.
            if let Some((s, e)) = selection {
                let (rs, re) = (line_start + start, line_start + end);
                let (from, to) = (s.max(rs), e.min(re));
                if from < to {
                    let x0 = x_of(from - line_start);
                    let x1 = x_of(to - line_start);
                    fill(
                        renderer,
                        Rectangle::new(Point::new(x0, y), Size::new(x1 - x0, row_h)),
                        selected,
                    );
                }
                let eol = line_start + chars.len();
                if end == chars.len() && s <= eol && e > eol {
                    fill(
                        renderer,
                        Rectangle::new(Point::new(x_of(chars.len()), y), Size::new(cell, row_h)),
                        selected,
                    );
                }
            }

            // The text, tabs drawn as spaces up to their stop.
            let mut content = String::new();
            for i in start..end {
                if chars[i] == '\t' {
                    content.push_str(&" ".repeat(vc[i + 1] - vc[i]));
                } else {
                    content.push(chars[i]);
                }
            }
            if !content.trim_end().is_empty() {
                renderer.fill_text(
                    self.text(content, row_h),
                    Point::new(left, y),
                    palette.text,
                    bounds,
                );
            }

            // The cursor.
            let on_row = line == cline && row == crow;
            if on_row {
                let x = x_of(ccol);
                let w = if ccol < chars.len() {
                    (vc[ccol + 1] - vc[ccol]) as f32 * cell
                } else {
                    cell
                };
                match self.view.mode {
                    Mode::Insert | Mode::CommandLine => {
                        fill(
                            renderer,
                            Rectangle::new(
                                Point::new(x, y + row_h * 0.12),
                                Size::new(2.0, row_h * 0.76),
                            ),
                            palette.primary,
                        );
                    }
                    Mode::Replace | Mode::OperatorPending => {
                        fill(
                            renderer,
                            Rectangle::new(
                                Point::new(x, y + row_h * 0.84),
                                Size::new(w, row_h * 0.1),
                            ),
                            palette.primary,
                        );
                    }
                    Mode::Normal | Mode::Visual | Mode::VisualLine | Mode::VisualBlock => {
                        fill(
                            renderer,
                            Rectangle::new(
                                Point::new(x, y + row_h * 0.1),
                                Size::new(w, row_h * 0.8),
                            ),
                            palette.primary,
                        );
                        if let Some(&c) = chars.get(ccol).filter(|c| !c.is_whitespace()) {
                            renderer.fill_text(
                                self.text(c.to_string(), row_h),
                                Point::new(x, y),
                                palette.background,
                                bounds,
                            );
                        }
                    }
                }
            }

            if row + 1 < rows.len() {
                row += 1;
            } else {
                line += 1;
                row = 0;
            }
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

impl<Message> Editor<'_, Message> {
    fn text(&self, content: String, row_h: f32) -> iced::advanced::Text<String, Font> {
        iced::advanced::Text {
            content,
            bounds: Size::new(f32::INFINITY, row_h),
            size: Pixels(self.size),
            line_height: text::LineHeight::Relative(LINE_HEIGHT),
            font: self.font,
            align_x: text::Alignment::Left,
            align_y: iced::alignment::Vertical::Top,
            shaping: text::Shaping::Advanced,
            wrapping: text::Wrapping::None,
        }
    }
}

fn fill(renderer: &mut iced::Renderer, bounds: Rectangle, color: Color) {
    renderer.fill_quad(
        Quad {
            bounds,
            ..Quad::default()
        },
        color,
    );
}

impl<'a, Message: 'a> From<Editor<'a, Message>> for Element<'a, Message> {
    fn from(editor: Editor<'a, Message>) -> Self {
        Element::new(editor)
    }
}
