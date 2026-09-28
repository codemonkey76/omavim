//! The editor: Omavim's own iced widget, because iced's `text_editor` can't
//! give Vim what it needs (a cursor per mode, visual selections, every key).
//!
//! It draws a centred column of soft-wrapped lines, the cursor (a block in
//! normal and visual mode, a bar in insert, an underline in replace and while
//! an operator waits), and the visual selection. What's shown is up to the
//! Vim engine, which wraps the lines and scrolls as Neovim does: this tells
//! it the size of the text area, draws from the row it says is at the top,
//! and hands every key press and wheel turn to the app.

use crate::colors::Style;
use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self, Quad, Renderer as _};
use iced::advanced::text::{self, Paragraph as _, Renderer as _};
use iced::advanced::widget::{self, Widget, tree};
use iced::advanced::{Clipboard, Shell};
use iced::keyboard;
use iced::mouse;
use iced::{Color, Element, Event, Font, Length, Pixels, Point, Rectangle, Size, Theme};
use omavim_vim::{Mode, Pos, wrap};
use ropey::Rope;
use std::cell::Cell;

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
    /// A visual block instead: its first and last lines, and its screen
    /// columns (both in; the end `usize::MAX` to each line's end).
    pub block: Option<(usize, usize, usize, usize)>,
    /// Search matches to highlight, and the one a search being typed would
    /// go to.
    pub matches: Vec<std::ops::Range<Pos>>,
    pub current_match: Option<std::ops::Range<Pos>>,
    pub tabstop: usize,
    /// The first screen row shown: a line, and a row of it.
    pub top: (usize, usize),
    /// Syntax highlighting: char ranges (in order) and how to draw them.
    pub highlights: std::rc::Rc<Vec<(std::ops::Range<Pos>, Style)>>,
}

pub struct Editor<'a, Message> {
    view: View<'a>,
    font: Font,
    size: f32,
    /// Cells in the text column, at most (0: as wide as the window).
    column: usize,
    on_key: Box<dyn Fn(KeyPress) -> Message + 'a>,
    /// The text area changed size: cells in a row, and rows.
    on_resize: Box<dyn Fn(usize, usize) -> Message + 'a>,
    /// The wheel turned: rows to scroll (down if positive).
    on_scroll: Box<dyn Fn(isize) -> Message + 'a>,
}

impl<'a, Message> Editor<'a, Message> {
    /// How to draw the char at `pos` (the highlight it's in).
    fn style_at(&self, pos: Pos) -> Style {
        let h = &self.view.highlights;
        let i = h.partition_point(|(r, _)| r.end <= pos);
        match h.get(i) {
            Some((r, s)) if r.start <= pos => *s,
            _ => Style::default(),
        }
    }

    pub fn new(
        view: View<'a>,
        font: Font,
        size: f32,
        on_key: impl Fn(KeyPress) -> Message + 'a,
        on_resize: impl Fn(usize, usize) -> Message + 'a,
        on_scroll: impl Fn(isize) -> Message + 'a,
    ) -> Self {
        Self {
            view,
            font,
            size,
            column: crate::config::COLUMN,
            on_key: Box::new(on_key),
            on_resize: Box::new(on_resize),
            on_scroll: Box::new(on_scroll),
        }
    }

    /// The text column's width in cells, at most (0: the window's).
    pub fn column(mut self, cells: usize) -> Self {
        self.column = cells;
        self
    }
}

#[derive(Default)]
struct State {
    /// One cell's width at the current size.
    cell: Cell<Option<(f32, f32)>>,
    /// The text area's size last told to the app: cells and rows.
    reported: Cell<Option<(usize, usize)>>,
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

    /// The text column in `bounds`: its left edge, its width in cells, and
    /// the rows that fit.
    fn geometry(&self, state: &State, bounds: Rectangle) -> (f32, usize, usize) {
        let cell = self.cell_width(state);
        let row_h = self.size * LINE_HEIGHT;
        let room = bounds.width - 2.0 * PADDING;
        let column_w = match self.column {
            0 => room,
            n => (cell * n as f32).min(room),
        }
        .max(cell * 10.0);
        let cells = (column_w / cell).floor() as usize;
        let left = bounds.x + ((bounds.width - cells as f32 * cell) / 2.0).max(0.0);
        let rows = (((bounds.height - 2.0 * PADDING) / row_h).floor() as usize).max(1);
        (left, cells, rows)
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
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<State>();
        // Tell the app (and so the engine) when the text area's size changes.
        let (_, cells, rows) = self.geometry(state, layout.bounds());
        if state.reported.get() != Some((cells, rows)) {
            state.reported.set(Some((cells, rows)));
            shell.publish((self.on_resize)(cells, rows));
        }
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
                let rows = rows.round() as isize;
                if rows != 0 {
                    shell.publish((self.on_scroll)(rows));
                }
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
        let (left, cells, visible) = self.geometry(state, bounds);

        let cursor = self.view.cursor.min(t.len_chars());
        let cline = t.char_to_line(cursor);
        let ccol = cursor - t.line_to_char(cline);
        let crow = wrap::layout(&line_chars(t, cline), cells, ts).row_of(ccol);

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
        let mut matched = palette.warning;
        matched.a = 0.30;
        let mut current = palette.warning;
        current.a = 0.65;

        let (mut line, mut row) = self.view.top;
        for screen_row in 0..visible {
            if line >= t.len_lines() {
                break;
            }
            let chars = line_chars(t, line);
            let lay = wrap::layout(&chars, cells, ts);
            let (vc, rows) = (&lay.vcols, &lay.rows);
            let row_i = row.min(rows.len() - 1);
            let (start, end) = rows[row_i];
            // Screen columns count from the line's start; this row's from
            // row_i rows of cells in.
            let base = row_i * cells;
            let y = bounds.y + PADDING + screen_row as f32 * row_h;
            let x_of =
                |col: usize| left + vc[col.min(chars.len())].saturating_sub(base) as f32 * cell;
            let line_start = t.line_to_char(line);

            // Search matches on this row.
            let (rs, re) = (line_start + start, line_start + end);
            let spans = self
                .view
                .matches
                .iter()
                .map(|m| (m, matched))
                .chain(self.view.current_match.iter().map(|m| (m, current)));
            for (m, color) in spans {
                let (from, to) = (m.start.max(rs), m.end.min(re));
                if from < to {
                    let x0 = x_of(from - line_start);
                    let x1 = x_of(to - line_start);
                    fill(
                        renderer,
                        Rectangle::new(Point::new(x0, y), Size::new(x1 - x0, row_h)),
                        color,
                    );
                }
            }

            // A block: the chars on this row in its screen columns.
            if let Some((top, bottom, bs, be)) = self.view.block
                && (top..=bottom).contains(&line)
            {
                let inside: Vec<usize> = (start..end)
                    .filter(|&i| vc[i] <= be && vc[i + 1] > bs)
                    .collect();
                if let (Some(&from), Some(&to)) = (inside.first(), inside.last()) {
                    let x0 = x_of(from);
                    let x1 = x_of(to + 1);
                    fill(
                        renderer,
                        Rectangle::new(Point::new(x0, y), Size::new(x1 - x0, row_h)),
                        selected,
                    );
                }
            }

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

            // The text, in runs of one style: tabs, and the gaps wrapping
            // leaves (a word moved to the next row, a wide char that didn't
            // fit), drawn as spaces.
            let mut runs: Vec<(usize, String, Style)> = Vec::new();
            let mut x = vc[start].saturating_sub(base);
            for i in start..end {
                let at = vc[i].saturating_sub(base);
                let style = self.style_at(line_start + i);
                if runs.last().is_none_or(|r| r.2 != style) {
                    runs.push((x, String::new(), style));
                }
                let content = &mut runs.last_mut().unwrap().1;
                while x < at {
                    content.push(' ');
                    x += 1;
                }
                // A tab is spaces to its stop; a wide char takes two cells.
                let (c, cells_used) = match chars[i] {
                    '\t' => (' ', 1),
                    c => (c, omavim_vim::text::char_width(c, 0, ts)),
                };
                content.push(c);
                x = at + cells_used;
                let next = vc[i + 1].saturating_sub(base);
                while x < next.min(cells) {
                    content.push(' ');
                    x += 1;
                }
            }
            for (at, content, style) in runs {
                if content.trim_end().is_empty() {
                    continue;
                }
                let x0 = left + at as f32 * cell;
                let color = style.color.unwrap_or(palette.text);
                if style.underline {
                    let width = content.trim_end().chars().count() as f32 * cell;
                    fill(
                        renderer,
                        Rectangle::new(Point::new(x0, y + row_h * 0.86), Size::new(width, 1.0)),
                        color,
                    );
                }
                let mut text = self.text(content, row_h);
                text.font = Font {
                    weight: if style.bold {
                        iced::font::Weight::Bold
                    } else {
                        iced::font::Weight::Normal
                    },
                    style: if style.italic {
                        iced::font::Style::Italic
                    } else {
                        iced::font::Style::Normal
                    },
                    ..self.font
                };
                renderer.fill_text(text, Point::new(x0, y), color, bounds);
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
                    Mode::Normal
                    | Mode::Visual
                    | Mode::VisualLine
                    | Mode::VisualBlock
                    | Mode::Confirm => {
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
