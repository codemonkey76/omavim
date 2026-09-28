//! Every case in tests/fixtures/cases.json, run through the engine and
//! compared with what Neovim did (tests/fixtures/expected.json). Regenerate
//! both with tools/fixtures/update.

use omavim_vim::{Mode, TextModel, Vim, key, text};
use ropey::Rope;
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Deserialize)]
struct Case {
    name: String,
    text: String,
    cursor: (usize, usize),
    keys: String,
    /// Compare the marks every edit moves too ('[ '] '. '^ '< '>).
    #[serde(default)]
    all_marks: bool,
}

#[derive(Deserialize, Debug, PartialEq, Clone)]
struct Expected {
    name: String,
    text: String,
    cursor: (usize, usize),
    mode: String,
    register: String,
    register_type: String,
    visual_start: Option<(usize, usize)>,
    /// The view: the first line shown and rows of it scrolled off.
    top: (usize, usize),
    /// The view before the keys (Neovim scrolls when the cursor is set).
    #[serde(default)]
    start_top: (usize, usize),
    /// The marks set: name → (line, column; 2147483647 for "the end").
    #[serde(default)]
    marks: BTreeMap<String, (usize, usize)>,
    /// The jump list and where CTRL-O is in it.
    #[serde(default)]
    jumps: (Vec<(usize, usize)>, usize),
    /// The other registers that aren't empty: name → (text, "v" or "V").
    #[serde(default)]
    registers: BTreeMap<String, (String, String)>,
    #[serde(default)]
    aborted: bool,
}

/// As Neovim's getregtype(): "v", "V", or CTRL-V and a block's width.
fn reg_type(reg: &omavim_vim::Register) -> String {
    match reg.block {
        Some(w) => format!("\u{16}{}", w + 1),
        None if reg.linewise => "V".into(),
        None => "v".into(),
    }
}

fn mode_name(m: Mode) -> &'static str {
    match m {
        Mode::Normal => "n",
        Mode::OperatorPending => "no",
        Mode::Insert => "i",
        Mode::Replace => "R",
        Mode::Visual => "v",
        Mode::VisualLine => "V",
        Mode::VisualBlock => "\u{16}",
        Mode::CommandLine => "c",
        Mode::Confirm => "r?",
    }
}

/// Run one case as typed: a key that fails beeps, and the keys after it
/// still run (Vim only drops keys that weren't typed, such as a mapping's).
/// Neovim's window: tools/fixtures/nvim.lua runs every case in one this size.
const WIDTH: usize = 30;
const HEIGHT: usize = 8;

fn run(case: &Case, start_top: (usize, usize)) -> Expected {
    let mut rope = Rope::from_str(&case.text);
    let mut vim = Vim::new();
    vim.set_screen(&rope, WIDTH, HEIGHT);
    // What tools/fixtures/nvim.lua leaves Neovim's last :s as.
    vim.set_last_substitute(r"\%^\%$", "", "e");
    // As Neovim does, a column past the line's end is its last char.
    let len = text::line_len(&rope, case.cursor.0);
    vim.set_cursor(text::pos(
        &rope,
        case.cursor.0,
        case.cursor.1.min(len.saturating_sub(1)),
    ));
    vim.set_top(&rope, start_top.0, start_top.1);
    vim.place_mark(
        &rope,
        '\'',
        (case.cursor.0, case.cursor.1.min(len.saturating_sub(1))),
    );
    let aborted = false;
    for k in key::parse(&case.keys) {
        let _ = vim.key(&mut rope, k);
    }
    let (line, col) = text::line_col(&rope, vim.cursor());
    let jumps = vim.jumplist(&rope);
    let reg = vim.register().clone();
    Expected {
        name: case.name.clone(),
        text: rope.to_string(),
        cursor: (line, col),
        top: vim.top(),
        start_top,
        marks: "abAB'[].^<>"
            .chars()
            .filter_map(|m| {
                let (l, c) = vim.get_mark(&rope, m)?;
                Some((m.to_string(), (l, c.min(2147483647))))
            })
            .collect(),
        jumps,
        mode: mode_name(vim.mode()).into(),
        register: reg.text.clone(),
        register_type: reg_type(&reg),
        visual_start: vim.visual_start().map(|p| text::line_col(&rope, p)),
        registers: "0123456789abcdefghijklmnopqrstuvwxyz-/"
            .chars()
            .filter_map(|r| {
                let reg = vim.get_register(r);
                (!reg.text.is_empty()).then(|| (r.to_string(), (reg.text.clone(), reg_type(reg))))
            })
            .collect(),
        aborted,
    }
}

/// Cases known to differ from Neovim, and why. Each is a corner of scrolling
/// inside a line taller than the window, where Neovim updates its view part
/// way through a command (as well as after it), which the engine doesn't
/// copy. Anything not listed must match exactly.
const KNOWN: &[(&str, &str)] = &[
    (
        "bri@6,20^(4, 1): H",
        "a view scrolled into a line before :set bri rewraps it: Neovim's skipcol",
    ),
    (
        "long@16,100: Vj>",
        "visual op from inside a tall line: view mid-command",
    ),
    (
        "long@16,100: vj>",
        "visual op from inside a tall line: view mid-command",
    ),
    (
        "long@16,100: v$y",
        "visual op from inside a tall line: view mid-command",
    ),
    (
        "long@16,100: yyo<C-r>0<Esc>",
        "inserting a tall line: view mid-insert",
    ),
    (
        "long@16,100: <C-v>jj$AX<Esc>",
        "appending at the end of a tall line, then back up it: view",
    ),
    (
        "markup@6,3: :g/e/j<CR>",
        "Neovim scrolls up a row after :g lengthens the part-skipped top line",
    ),
    (
        "long@16,200^(16, 5): <C-b><C-b>",
        "CTRL-B twice from a tall line: cursor column",
    ),
];

#[test]
fn behaves_like_neovim() {
    let cases: Vec<Case> = serde_json::from_str(include_str!("fixtures/cases.json")).unwrap();
    let expected: Vec<Expected> =
        serde_json::from_str(include_str!("fixtures/expected.json")).unwrap();
    assert_eq!(cases.len(), expected.len());
    let mut failures = Vec::new();
    for (case, want) in cases.iter().zip(&expected) {
        let mut got = run(case, want.start_top);
        let mut want = want.clone();
        if !case.all_marks {
            for m in "[].^<>".chars() {
                want.marks.remove(&m.to_string());
                got.marks.remove(&m.to_string());
            }
        }
        if got != want && !KNOWN.iter().any(|(name, _)| *name == case.name) {
            failures.push((case, want, got));
        }
    }
    if !failures.is_empty() {
        let show = std::env::var("SHOW")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(15);
        let filter = std::env::var("ONLY").unwrap_or_default();
        let mut shown = 0;
        for (case, want, got) in &failures {
            if !case.name.contains(&filter) || shown >= show {
                continue;
            }
            shown += 1;
            eprintln!("── {}", case.name);
            if want.text != got.text {
                eprintln!(
                    "   text  want {:?}\n         got  {:?}",
                    want.text, got.text
                );
            }
            if want.top != got.top {
                eprintln!("   top   want {:?} got {:?}", want.top, got.top);
            }
            if want.cursor != got.cursor {
                eprintln!("   cursor want {:?} got {:?}", want.cursor, got.cursor);
            }
            if want.mode != got.mode {
                eprintln!("   mode  want {} got {}", want.mode, got.mode);
            }
            if (&want.register, &want.register_type) != (&got.register, &got.register_type) {
                eprintln!(
                    "   reg   want {:?}{} got {:?}{}",
                    want.register, want.register_type, got.register, got.register_type
                );
            }
            if want.registers != got.registers {
                eprintln!(
                    "   regs  want {:?}\n         got  {:?}",
                    want.registers, got.registers
                );
            }
            if want.marks != got.marks {
                eprintln!(
                    "   marks want {:?}\n         got  {:?}",
                    want.marks, got.marks
                );
            }
            if want.jumps != got.jumps {
                eprintln!(
                    "   jumps want {:?}\n         got  {:?}",
                    want.jumps, got.jumps
                );
            }
            if want.visual_start != got.visual_start {
                eprintln!(
                    "   visual want {:?} got {:?}",
                    want.visual_start, got.visual_start
                );
            }
            if want.aborted != got.aborted {
                eprintln!("   aborted want {} got {}", want.aborted, got.aborted);
            }
        }
        panic!(
            "{} of {} cases differ from Neovim",
            failures.len(),
            cases.len()
        );
    }
}

#[allow(dead_code)]
fn _uses(_: &dyn TextModel) {}
