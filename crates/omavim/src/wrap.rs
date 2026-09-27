//! Soft wrapping: a line split into screen rows at the last space that fits,
//! measured in screen cells (tabs to their tab stop, wide chars two).

use omavim_vim::text::char_width;

/// Screen cells before each char of a line, and after the last: `vcols[i]` is
/// where char `i` starts; `vcols[len]` is the line's width.
pub fn vcols(chars: &[char], tabstop: usize) -> Vec<usize> {
    let mut out = Vec::with_capacity(chars.len() + 1);
    let mut v = 0;
    for &c in chars {
        out.push(v);
        v += char_width(c, v, tabstop);
    }
    out.push(v);
    out
}

/// The rows of a line `width` cells wide, as char ranges. Always at least one
/// row, so an empty line still takes a row.
pub fn rows(chars: &[char], width: usize, tabstop: usize) -> Vec<(usize, usize)> {
    let width = width.max(1);
    let vc = vcols(chars, tabstop);
    let mut rows = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let mut end = start;
        let mut last_break = None;
        while end < chars.len() && vc[end + 1] - vc[start] <= width {
            if chars[end] == ' ' || chars[end] == '\t' {
                last_break = Some(end + 1);
            }
            end += 1;
        }
        if end < chars.len() {
            // Too long: break after the last blank, or mid-word if there's none.
            if let Some(b) = last_break.filter(|&b| b > start) {
                end = b;
            }
            if end == start {
                end = start + 1;
            }
        }
        rows.push((start, end));
        start = end;
    }
    if rows.is_empty() {
        rows.push((0, 0));
    }
    rows
}

/// Which row a column is on (the end of the line is on the last row).
pub fn row_of(rows: &[(usize, usize)], col: usize) -> usize {
    rows.iter()
        .position(|&(s, e)| col >= s && col < e)
        .unwrap_or(rows.len() - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrap(s: &str, width: usize) -> Vec<String> {
        let chars: Vec<char> = s.chars().collect();
        rows(&chars, width, 8)
            .into_iter()
            .map(|(a, b)| chars[a..b].iter().collect())
            .collect()
    }

    #[test]
    fn breaks_after_the_last_space_that_fits() {
        assert_eq!(wrap("the quick brown fox", 10), ["the quick ", "brown fox"]);
        assert_eq!(wrap("short", 10), ["short"]);
        assert_eq!(wrap("", 10), [""]);
    }

    #[test]
    fn breaks_a_word_too_long_for_a_row() {
        assert_eq!(wrap("abcdefghijkl", 5), ["abcde", "fghij", "kl"]);
    }

    #[test]
    fn measures_tabs_and_wide_chars_in_cells() {
        assert_eq!(vcols(&['\t', 'a'], 8), [0, 8, 9]);
        assert_eq!(vcols(&['日', 'a'], 8), [0, 2, 3]);
        assert_eq!(wrap("日本語日本語", 6), ["日本語", "日本語"]);
    }

    #[test]
    fn finds_a_columns_row() {
        let r = [(0, 10), (10, 19)];
        assert_eq!(row_of(&r, 3), 0);
        assert_eq!(row_of(&r, 12), 1);
        assert_eq!(row_of(&r, 19), 1);
    }
}
