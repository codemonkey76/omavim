//! Bold, italic and links: Omavim's own, not Vim's.

use omavim_vim::key::parse;
use omavim_vim::{Mode, Vim};
use ropey::Rope;

fn run(text: &str, at: usize, keys: &str, then: impl FnOnce(&mut Vim, &mut Rope)) -> (String, Vim) {
    let mut rope = Rope::from_str(text);
    let mut vim = Vim::new();
    vim.set_cursor(at);
    for k in parse(keys) {
        let _ = vim.key(&mut rope, k);
    }
    then(&mut vim, &mut rope);
    (rope.to_string(), vim)
}

#[test]
fn bold_goes_around_the_word_and_comes_off_again() {
    let (text, vim) = run("some words here", 6, "", |v, r| {
        v.surround(r, "**").unwrap()
    });
    assert_eq!(text, "some **words** here");
    assert_eq!(vim.cursor(), 5);
    let (text, _) = run("some **words** here", 8, "", |v, r| {
        v.surround(r, "**").unwrap()
    });
    assert_eq!(text, "some words here");
}

#[test]
fn around_a_selection_then_one_undo() {
    let (text, vim) = run("some words here", 0, "vee", |v, r| {
        v.surround(r, "_").unwrap()
    });
    assert_eq!(text, "_some words_ here");
    assert_eq!(vim.mode(), Mode::Normal);
    let (text, _) = run("some words here", 0, "vee", |v, r| {
        v.surround(r, "_").unwrap();
        for k in parse("u") {
            let _ = v.key(r, k);
        }
    });
    assert_eq!(text, "some words here");
    // A selection that is the bold text, marks and all: off.
    let (text, _) = run("a **b c** d", 2, "v6l", |v, r| v.surround(r, "**").unwrap());
    assert_eq!(text, "a b c d");
}

#[test]
fn on_a_blank_it_types_between_a_pair() {
    let (text, vim) = run("", 0, "", |v, r| {
        v.surround(r, "**").unwrap();
        for k in parse("bold<Esc>") {
            let _ = v.key(r, k);
        }
    });
    assert_eq!(text, "**bold**");
    assert_eq!(vim.mode(), Mode::Normal);
}

#[test]
fn a_link_types_the_address() {
    let (text, vim) = run("see the docs", 9, "", |v, r| {
        v.link(r).unwrap();
        for k in parse("https://x.y<Esc>") {
            let _ = v.key(r, k);
        }
    });
    assert_eq!(text, "see the [docs](https://x.y)");
    assert_eq!(vim.mode(), Mode::Normal);
    // And undoes in one.
    let (text, _) = run("see the docs", 9, "", |v, r| {
        v.link(r).unwrap();
        for k in parse("https://x.y<Esc>u") {
            let _ = v.key(r, k);
        }
    });
    assert_eq!(text, "see the docs");
}

#[test]
fn a_reload_is_one_undo_and_keeps_the_cursor() {
    let (text, vim) = run("one\ntwo\nthree\n", 9, "", |v, r| {
        v.reload(r, "one\nTWO\nthree\nfour\n");
    });
    assert_eq!(text, "one\nTWO\nthree\nfour\n");
    assert_eq!(vim.cursor(), 9, "still on three");
    let (text, _) = run("one\ntwo\n", 0, "", |v, r| {
        v.reload(r, "one\nTWO\nthree\n");
        for k in parse("u") {
            let _ = v.key(r, k);
        }
    });
    assert_eq!(text, "one\ntwo\n");
}
