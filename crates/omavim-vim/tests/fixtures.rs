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
}

#[derive(Deserialize, Debug, PartialEq)]
struct Expected {
    name: String,
    text: String,
    cursor: (usize, usize),
    mode: String,
    register: String,
    register_type: String,
    visual_start: Option<(usize, usize)>,
    /// The other registers that aren't empty: name → (text, "v" or "V").
    #[serde(default)]
    registers: BTreeMap<String, (String, String)>,
    #[serde(default)]
    aborted: bool,
}

fn mode_name(m: Mode) -> &'static str {
    match m {
        Mode::Normal | Mode::OperatorPending => "n",
        Mode::Insert => "i",
        Mode::Replace => "R",
        Mode::Visual => "v",
        Mode::VisualLine => "V",
        Mode::VisualBlock => "\u{16}",
        Mode::CommandLine => "c",
    }
}

/// Run one case as typed: a key that fails beeps, and the keys after it
/// still run (Vim only drops keys that weren't typed, such as a mapping's).
fn run(case: &Case) -> Expected {
    let mut rope = Rope::from_str(&case.text);
    let mut vim = Vim::new();
    vim.set_cursor(text::pos(&rope, case.cursor.0, case.cursor.1));
    let aborted = false;
    for k in key::parse(&case.keys) {
        let _ = vim.key(&mut rope, k);
    }
    let (line, col) = text::line_col(&rope, vim.cursor());
    let reg = vim.register();
    Expected {
        name: case.name.clone(),
        text: rope.to_string(),
        cursor: (line, col),
        mode: mode_name(vim.mode()).into(),
        register: reg.text.clone(),
        register_type: if reg.linewise { "V".into() } else { "v".into() },
        visual_start: vim.visual_start().map(|p| text::line_col(&rope, p)),
        registers: "0123456789abcdefghijklmnopqrstuvwxyz-/"
            .chars()
            .filter_map(|r| {
                let reg = vim.get_register(r);
                (!reg.text.is_empty()).then(|| {
                    let kind = if reg.linewise { "V" } else { "v" };
                    (r.to_string(), (reg.text.clone(), kind.to_string()))
                })
            })
            .collect(),
        aborted,
    }
}

#[test]
fn behaves_like_neovim() {
    let cases: Vec<Case> = serde_json::from_str(include_str!("fixtures/cases.json")).unwrap();
    let expected: Vec<Expected> =
        serde_json::from_str(include_str!("fixtures/expected.json")).unwrap();
    assert_eq!(cases.len(), expected.len());
    let mut failures = Vec::new();
    for (case, want) in cases.iter().zip(&expected) {
        let got = run(case);
        if got != *want {
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
