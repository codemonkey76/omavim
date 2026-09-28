//! Text objects from the syntax tree.

use omavim_syntax::{Lang, Syntax};
use ropey::Rope;

fn objects(lang: Lang, text: &str, kind: &str, inner: bool, near: &str) -> Vec<String> {
    let rope = Rope::from_str(text);
    let mut s = Syntax::new(lang);
    s.parse(&rope);
    let near = text.find(near).unwrap();
    let mut found: Vec<String> = s
        .objects(&rope, kind, inner, near)
        .into_iter()
        .map(|r| text[r].to_string())
        .collect();
    found.sort();
    found.dedup();
    found
}

const RUST: &str =
    "struct S {\n    a: u32,\n}\n\n/// Adds.\nfn add(a: u32, b: u32) -> u32 {\n    a + b\n}\n";

#[test]
fn rust() {
    assert_eq!(
        objects(Lang::Rust, RUST, "function", false, "a + b"),
        ["fn add(a: u32, b: u32) -> u32 {\n    a + b\n}"]
    );
    assert_eq!(
        objects(Lang::Rust, RUST, "function", true, "a + b"),
        ["a + b"]
    );
    assert_eq!(objects(Lang::Rust, RUST, "class", true, "a"), ["a: u32,"]);
    assert_eq!(
        objects(Lang::Rust, RUST, "parameter", false, "a + b"),
        [", b: u32", "a: u32,"]
    );
    assert_eq!(
        objects(Lang::Rust, RUST, "comment", false, "a"),
        ["/// Adds.\n"]
    );
}

#[test]
fn python_comments_leave_the_space() {
    assert!(
        objects(Lang::Python, "# hello\nx = 1\n", "comment", true, "x")
            .contains(&"hello".to_string())
    );
}

const MD: &str = "# One\n\nSome *em* and **strong**, [a link](http://x) and `code`.\n\n## Two\n\nText.\n\n```rust\nfn f(x: u8) {}\n```\n";

#[test]
fn markdown() {
    let near = "em*";
    assert_eq!(
        objects(Lang::Markdown, MD, "emphasis", true, near),
        ["em", "strong"]
    );
    assert_eq!(
        objects(Lang::Markdown, MD, "emphasis", false, near),
        ["**strong**", "*em*"]
    );
    assert_eq!(objects(Lang::Markdown, MD, "link", true, near), ["a link"]);
    assert_eq!(
        objects(Lang::Markdown, MD, "code", true, near),
        ["code", "fn f(x: u8) {}\n"]
    );
    assert_eq!(
        objects(Lang::Markdown, MD, "heading", true, near),
        [
            "Some *em* and **strong**, [a link](http://x) and `code`.\n\n## Two\n\nText.\n\n```rust\nfn f(x: u8) {}\n```\n",
            "Text.\n\n```rust\nfn f(x: u8) {}\n```\n"
        ]
    );
}

#[test]
fn code_in_markdown_where_the_cursor_is() {
    assert_eq!(
        objects(Lang::Markdown, MD, "parameter", true, "x: u8"),
        ["x: u8"]
    );
    assert!(objects(Lang::Markdown, MD, "parameter", true, "Text").is_empty());
}

#[test]
fn strings_and_comments() {
    let text = "fn f() { let s = \"(\"; } // (x\n";
    let rope = Rope::from_str(text);
    let mut s = Syntax::new(Lang::Rust);
    s.parse(&rope);
    let at = |what: &str| s.region(text.find(what).unwrap()).map(|r| &text[r]);
    assert_eq!(at("(\";"), Some("\"(\""));
    assert_eq!(at("\"(\""), Some("\"(\""));
    assert_eq!(at("(x"), Some("// (x"));
    assert_eq!(at("let"), None);
}
