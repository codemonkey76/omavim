//! Soft wrapping, as Vim does it with 'wrap' and 'linebreak': a line is cut
//! into screen rows `width` cells wide, and a word that won't fit is moved to
//! the next row by widening the break char before it to the row's end.

use crate::text::char_width;

/// Vim's default 'breakat': a row may end after one of these.
pub const BREAKAT: &str = " \t!@*-+;:,./?";

/// How lines wrap: Vim's 'breakindent' and 'breakat' as well as the width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wrap<'a> {
    /// Cells in a row (0: no wrapping).
    pub width: usize,
    pub tabstop: usize,
    /// Rows after the first start as far in as the line (Vim's
    /// 'breakindent', with its default 'breakindentopt').
    pub breakindent: bool,
    /// Where a row may end ('breakat').
    pub breakat: &'a str,
}

impl Wrap<'_> {
    /// How far in rows after the first start: the line's indent, less what
    /// would leave fewer than 20 cells (Vim's `min:20`).
    pub fn indent(&self, chars: &[char]) -> usize {
        if !self.breakindent || self.width == 0 {
            return 0;
        }
        let mut v = 0;
        for &c in chars.iter().take_while(|&&c| c == ' ' || c == '\t') {
            v += char_width(c, v, self.tabstop);
        }
        v.min(self.width.saturating_sub(20))
    }
}

fn is_wide(c: char) -> bool {
    c != '\t' && char_width(c, 0, 8) == 2
}

/// A line laid out on screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    /// The virtual column each char starts at, and (last) where the line
    /// ends. With wrapping, these count the padding before a moved word, so
    /// row `r` starts at column `r * width`.
    pub vcols: Vec<usize>,
    /// Each screen row's chars, as a range. Always at least one row.
    pub rows: Vec<(usize, usize)>,
}

impl Layout {
    /// The row a char is on (the line's end is on the last row).
    pub fn row_of(&self, col: usize) -> usize {
        self.rows
            .iter()
            .position(|&(s, e)| col >= s && col < e)
            .unwrap_or(self.rows.len() - 1)
    }
}

/// Lay a line out in rows `width` cells wide (0: no wrapping), as Neovim
/// does with 'linebreak' and the default 'breakat' (its charsize_regular()).
pub fn layout(chars: &[char], width: usize, tabstop: usize) -> Layout {
    layout_with(
        chars,
        &Wrap {
            width,
            tabstop,
            breakindent: false,
            breakat: BREAKAT,
        },
    )
}

/// Lay a line out as `wrap` says; with 'breakindent', the cells before each
/// later row's first char are counted in its column, as padding is.
pub fn layout_with(chars: &[char], wrap: &Wrap) -> Layout {
    let Wrap { width, tabstop, .. } = *wrap;
    let is_break = |c: char| wrap.breakat.contains(c);
    let indent = wrap.indent(chars);
    let mut vcols = Vec::with_capacity(chars.len() + 1);
    // No break in the break chars a line starts with (its indent, `// `).
    let lead = chars.iter().take_while(|&&c| is_break(c)).count();
    let mut v = 0;
    for (i, &c) in chars.iter().enumerate() {
        if indent > 0 && v > 0 && v % width == 0 {
            v += indent;
        }
        vcols.push(v);
        let mut size = char_width(c, v, tabstop);
        if width > 0 {
            // A wide char that would start in a row's last cell goes to the
            // next row (in as far as that row starts), leaving a filler cell.
            if size == 2 && is_wide(c) && v % width == width - 1 {
                let pad = 1 + indent;
                size += pad;
                *vcols.last_mut().unwrap() += pad;
            }
            // At a break char before a word: if the word and the break chars
            // after it don't fit, pad to the row's end so it starts the next
            // row. (Measured as Vim does, from this char's column, which is
            // how a tab here gets its odd sizes.)
            if i >= lead && is_break(c) && chars.get(i + 1).is_some_and(|&n| !is_break(n)) {
                // In signed arithmetic, as Vim's C.
                let (w, vi) = (width as isize, v as isize);
                let col_adj = size as isize - 1;
                let mut colmax = w - col_adj;
                if vi >= colmax {
                    colmax += col_adj;
                    colmax += ((vi - colmax) / w + 1) * w - col_adj;
                }
                let mut v2 = vi;
                for j in i + 1..chars.len() {
                    let (cj, prev) = (chars[j], chars[j - 1]);
                    if !(is_break(cj) || v2 == vi || !is_break(prev)) {
                        break;
                    }
                    v2 += char_width(cj, v2 as usize, tabstop) as isize;
                    if v2 >= colmax {
                        size = (colmax - vi + col_adj).max(1) as usize;
                        break;
                    }
                }
            }
        }
        v += size;
    }
    vcols.push(v);
    let mut rows = Vec::new();
    if width == 0 || chars.is_empty() {
        rows.push((0, chars.len()));
    } else {
        let mut start = 0;
        for i in 1..chars.len() {
            if vcols[i] / width != vcols[start] / width {
                rows.push((start, i));
                start = i;
            }
        }
        rows.push((start, chars.len()));
    }
    Layout { vcols, rows }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrap(s: &str, width: usize) -> Vec<String> {
        let chars: Vec<char> = s.chars().collect();
        layout(&chars, width, 8)
            .rows
            .into_iter()
            .map(|(a, b)| chars[a..b].iter().collect())
            .collect()
    }

    #[test]
    fn moves_a_word_that_wont_fit() {
        assert_eq!(
            wrap("the quick brown fox jumps over", 20),
            ["the quick brown fox ", "jumps over"]
        );
        // The word and the blanks after it must fit.
        assert_eq!(
            wrap("twenty chars then sp  x", 20),
            ["twenty chars then ", "sp  x"]
        );
        assert_eq!(wrap("exactly twenty chars", 20), ["exactly twenty chars"]);
        assert_eq!(wrap("", 20), [""]);
    }

    #[test]
    fn cuts_a_word_too_long_for_a_row() {
        assert_eq!(wrap("abcdefghijkl", 5), ["abcde", "fghij", "kl"]);
    }

    #[test]
    fn counts_padding_in_the_columns() {
        let chars: Vec<char> = "ab cd".chars().collect();
        assert_eq!(layout(&chars, 4, 8).vcols, [0, 1, 2, 4, 5, 6]);
    }
}
