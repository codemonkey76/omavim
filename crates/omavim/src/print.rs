//! Printing: the document laid out on pages, in points at the paper's own
//! size, into a PDF that the Print portal hands to the system's dialog.
//! 12pt text (Omawrite printed at a fraction of that: its issue #29), in iA
//! Writer Mono, wrapped at word breaks as on screen, bold and italic where
//! the highlighting has them, black on white.

use omavim_vim::wrap;
use ropey::Rope;
use std::ops::Range;

/// The text size, in points.
pub const SIZE: f32 = 12.0;
/// Line height, relative to the text size.
const LINE: f32 = 1.45;
/// iA Writer Mono's advance: 600 units of 1000 to the em.
const ADVANCE: f32 = 0.6;
/// The least margin, whatever the printer says it can reach: 20mm.
const MIN_MARGIN: f32 = 20.0 * MM;
const MM: f32 = 72.0 / 25.4;

/// A page's size and margins, in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Paper {
    pub width: f32,
    pub height: f32,
    /// Top, right, bottom, left.
    pub margins: [f32; 4],
}

impl Paper {
    /// A4 portrait.
    #[cfg(test)]
    pub fn a4() -> Self {
        Self::from_mm(210.0, 297.0, [0.0; 4])
    }

    /// From the print dialog's millimetres (margins at least 20mm).
    pub fn from_mm(width: f64, height: f64, margins: [f64; 4]) -> Self {
        Self {
            width: width as f32 * MM,
            height: height as f32 * MM,
            margins: margins.map(|m| (m as f32 * MM).max(MIN_MARGIN)),
        }
    }

    /// Characters in a line, and lines on a page.
    pub fn grid(&self) -> (usize, usize) {
        let [top, right, bottom, left] = self.margins;
        let cols = ((self.width - left - right) / (SIZE * ADVANCE))
            .floor()
            .max(1.0);
        let rows = ((self.height - top - bottom) / (SIZE * LINE))
            .floor()
            .max(1.0);
        (cols as usize, rows as usize)
    }
}

/// How a stretch of text is set.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Face {
    pub bold: bool,
    pub italic: bool,
}

/// A piece of a printed line: where it starts (in characters from the
/// margin), its text, and its face.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub col: usize,
    pub text: String,
    pub face: Face,
}

/// Pages of lines of runs.
pub type Pages = Vec<Vec<Vec<Run>>>;

/// Lay the text out on pages, wrapped as `wrapping` says (at the paper's
/// width): `faces` are sorted char ranges.
pub fn layout(
    text: &Rope,
    faces: &[(Range<usize>, Face)],
    paper: &Paper,
    wrapping: &wrap::Wrap,
) -> Pages {
    let (cols, rows) = paper.grid();
    let wrapping = wrap::Wrap {
        width: cols,
        ..*wrapping
    };
    let mut lines: Vec<Vec<Run>> = Vec::new();
    let mut at = 0;
    let mut f = 0;
    for line in text.lines() {
        let chars: Vec<char> = line.chars().filter(|&c| c != '\n' && c != '\r').collect();
        let l = wrap::layout_with(&chars, &wrapping);
        let mut printed: Vec<Vec<Run>> = vec![Vec::new(); l.rows.len().max(1)];
        for (i, &c) in chars.iter().enumerate() {
            let pos = at + i;
            while f < faces.len() && faces[f].0.end <= pos {
                f += 1;
            }
            let face = faces
                .get(f)
                .filter(|(r, _)| r.start <= pos)
                .map(|(_, face)| *face)
                .unwrap_or_default();
            let (row, col) = (l.vcols[i] / cols, l.vcols[i] % cols);
            if c == '\t' || c == ' ' {
                continue;
            }
            let last = printed.len() - 1;
            let runs = &mut printed[row.min(last)];
            match runs.last_mut() {
                Some(r) if r.face == face && r.col + r.text.chars().count() == col => {
                    r.text.push(c)
                }
                _ => runs.push(Run {
                    col,
                    text: c.to_string(),
                    face,
                }),
            }
        }
        lines.extend(printed);
        at += line.len_chars();
    }
    // (Ropey's last line is empty after a final line break: not a line.)
    if text.len_chars() > 0 && text.char(text.len_chars() - 1) == '\n' {
        lines.pop();
    }
    let mut pages: Pages = lines.chunks(rows).map(<[_]>::to_vec).collect();
    if pages.is_empty() {
        pages.push(Vec::new());
    }
    pages
}

/// The pages as a PDF.
pub fn pdf(pages: &Pages, paper: &Paper, title: &str) -> Result<Vec<u8>, String> {
    use krilla::Document;
    use krilla::geom::Point;
    use krilla::page::PageSettings;
    use krilla::text::{Font, TextDirection};
    let font = |data: &'static [u8]| Font::new(data.into(), 0).ok_or("the font didn't load");
    let regular = font(include_bytes!("../../../fonts/iAWriterMonoS-Regular.ttf"))?;
    let bold = font(include_bytes!("../../../fonts/iAWriterMonoS-Bold.ttf"))?;
    let italic = font(include_bytes!("../../../fonts/iAWriterMonoS-Italic.ttf"))?;
    let bold_italic = font(include_bytes!(
        "../../../fonts/iAWriterMonoS-BoldItalic.ttf"
    ))?;
    // (Uncompressed in tests, to read the pages back.)
    let mut doc = Document::new_with(krilla::SerializeSettings {
        compress_content_streams: !cfg!(test),
        ..Default::default()
    });
    doc.set_metadata(krilla::metadata::Metadata::new().title(title.to_string()));
    let [top, _, _, left] = paper.margins;
    for page in pages {
        let settings = PageSettings::from_wh(paper.width, paper.height).ok_or("no paper size")?;
        let mut p = doc.start_page_with(settings);
        let mut surface = p.surface();
        for (row, runs) in page.iter().enumerate() {
            // The baseline: a line's height down, less what's below it.
            let y = top + (row as f32 + 1.0) * SIZE * LINE - SIZE * (LINE - 1.0) / 2.0 - SIZE * 0.2;
            for run in runs {
                let font = match (run.face.bold, run.face.italic) {
                    (false, false) => &regular,
                    (true, false) => &bold,
                    (false, true) => &italic,
                    (true, true) => &bold_italic,
                };
                surface.draw_text(
                    Point::from_xy(left + run.col as f32 * SIZE * ADVANCE, y),
                    font.clone(),
                    SIZE,
                    &run.text,
                    false,
                    TextDirection::Auto,
                );
            }
        }
        surface.finish();
        p.finish();
    }
    doc.finish().map_err(|e| format!("{e:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: wrap::Wrap = wrap::Wrap {
        width: 0,
        tabstop: 8,
        breakindent: false,
        breakat: wrap::BREAKAT,
    };

    fn runs(pages: &Pages) -> Vec<Vec<String>> {
        pages[0]
            .iter()
            .map(|l| l.iter().map(|r| format!("{}:{}", r.col, r.text)).collect())
            .collect()
    }

    #[test]
    fn a4_at_12pt_fits_a_writers_line() {
        let (cols, rows) = Paper::a4().grid();
        // 170mm of text at 7.2pt a character; 257mm at 17.4pt a line.
        assert_eq!((cols, rows), (66, 41));
    }

    #[test]
    fn wraps_at_word_breaks_and_pages() {
        let paper = Paper {
            // (A point over: exactly 10 and 3 by the sums, not quite by floats.)
            width: 10.0 * SIZE * ADVANCE + 2.0 * MIN_MARGIN + 1.0,
            height: 3.0 * SIZE * LINE + 2.0 * MIN_MARGIN + 1.0,
            margins: [MIN_MARGIN; 4],
        };
        let text = Rope::from_str("one two three four\nfive\n\nsix\n");
        let pages = layout(&text, &[], &paper, &PLAIN);
        assert_eq!(
            runs(&pages),
            [
                vec!["0:one", "4:two"],
                vec!["0:three", "6:four"],
                vec!["0:five"]
            ]
        );
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[1].len(), 2, "the empty line, six");
    }

    #[test]
    fn faces_split_runs() {
        let text = Rope::from_str("a **b** c");
        let faces = [(
            2..7,
            Face {
                bold: true,
                italic: false,
            },
        )];
        let pages = layout(&text, &faces, &Paper::a4(), &PLAIN);
        let got: Vec<(String, bool)> = pages[0][0]
            .iter()
            .map(|r| (r.text.clone(), r.face.bold))
            .collect();
        assert_eq!(
            got,
            [
                ("a".into(), false),
                ("**b**".into(), true),
                ("c".into(), false)
            ]
        );
    }

    /// A look at a real one, by hand: `OMAVIM_SAMPLE_PDF=out.pdf cargo test
    /// -p omavim sample -- --ignored` prints the README.
    #[test]
    #[ignore]
    fn sample() {
        let Some(out) = std::env::var_os("OMAVIM_SAMPLE_PDF") else {
            return;
        };
        let text = Rope::from_str(include_str!("../../../README.md"));
        let mut syntax = omavim_syntax::Syntax::new(omavim_syntax::Lang::Markdown);
        syntax.parse(&text);
        let colors = crate::colors::Colors::builtin(crate::portal::Scheme::Light);
        let faces: Vec<(Range<usize>, Face)> = syntax
            .highlights(&text, 0..text.len_bytes())
            .into_iter()
            .map(|s| (s.range, colors.style(s.name)))
            .filter(|(_, st)| st.bold || st.italic)
            .map(|(r, st)| {
                (
                    text.byte_to_char(r.start)..text.byte_to_char(r.end),
                    Face {
                        bold: st.bold,
                        italic: st.italic,
                    },
                )
            })
            .collect();
        let paper = Paper::a4();
        std::fs::write(
            out,
            pdf(&layout(&text, &faces, &paper, &PLAIN), &paper, "README.md").unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn the_pdf_is_a4_with_12pt_text() {
        let text = Rope::from_str("# Title\n\nSome words.\n");
        let paper = Paper::a4();
        let bytes = pdf(&layout(&text, &[], &paper, &PLAIN), &paper, "notes.md").unwrap();
        let pdf = String::from_utf8_lossy(&bytes);
        assert!(pdf.starts_with("%PDF-"));
        // A4 is 595.28 by 841.89 points.
        assert!(pdf.contains("/MediaBox[0 0 595.27563 841.8898]"));
        assert!(pdf.contains(" 12 Tf"), "the text is set at 12pt");
        assert!(!pdf.contains(" 1.5 Tf"));
    }
}
