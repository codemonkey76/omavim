//! Soft wrapping checked against Neovim: where each char of a line lands on
//! screen (row, column) in windows of several widths, with 'linebreak' on.
//! Regenerate with tools/fixtures/update.

use omavim_vim::wrap::{BREAKAT, Wrap, layout_with};
use serde::Deserialize;

#[derive(Deserialize)]
struct Case {
    line: String,
    width: usize,
    #[serde(default)]
    breakindent: bool,
    breakat: Option<String>,
}

#[derive(Deserialize)]
struct Expected {
    width: usize,
    cells: Vec<(usize, usize)>,
}

#[test]
fn wraps_like_neovim() {
    let cases: Vec<Case> = serde_json::from_str(include_str!("fixtures/wrap_cases.json")).unwrap();
    let expected: Vec<Expected> =
        serde_json::from_str(include_str!("fixtures/wrap_expected.json")).unwrap();
    let mut failures = 0;
    for (case, want) in cases.iter().zip(&expected) {
        assert_eq!(
            case.width, want.width,
            "Neovim's window wasn't the width asked for"
        );
        let chars: Vec<char> = case.line.chars().collect();
        let l = layout_with(
            &chars,
            &Wrap {
                width: case.width,
                tabstop: 8,
                breakindent: case.breakindent,
                breakat: case.breakat.as_deref().unwrap_or(BREAKAT),
            },
        );
        let got: Vec<(usize, usize)> = (0..chars.len())
            .map(|i| (l.vcols[i] / case.width, l.vcols[i] % case.width))
            .collect();
        if got != want.cells {
            failures += 1;
            if failures <= 5 {
                let at = got
                    .iter()
                    .zip(&want.cells)
                    .position(|(a, b)| a != b)
                    .unwrap_or(0);
                eprintln!(
                    "── width {}: {:?}\n   first difference at char {at} {:?}: want {:?} got {:?}",
                    case.width,
                    case.line,
                    chars.get(at),
                    want.cells.get(at),
                    got.get(at)
                );
            }
        }
    }
    assert_eq!(
        failures,
        0,
        "{failures} of {} lines wrap differently",
        cases.len()
    );
}
