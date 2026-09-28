//! Every bundled language loads, and highlighting finds what it should.

use omavim_syntax::{Lang, Span, Syntax};
use ropey::Rope;

fn highlight(lang: Lang, text: &str) -> (Rope, Vec<Span>) {
    let rope = Rope::from_str(text);
    let mut s = Syntax::new(lang);
    s.parse(&rope);
    let spans = s.highlights(&rope, 0..rope.len_bytes());
    (rope, spans)
}

/// The highlight name on the first occurrence of `needle`.
fn name_at(rope: &Rope, spans: &[Span], needle: &str) -> Option<&'static str> {
    let at = rope.to_string().find(needle).expect("needle in text");
    spans.iter().find(|s| s.range.contains(&at)).map(|s| s.name)
}

#[test]
fn every_language_loads() {
    for lang in Lang::ALL {
        let (_, _) = highlight(lang, "x");
    }
}

#[test]
fn markdown_and_the_code_in_it() {
    let text = "# Title\n\nSome *emphasis* and `code`.\n\n```rust\nfn main() { let x = 1; }\n```\n";
    let (rope, spans) = highlight(Lang::Markdown, text);
    assert_eq!(name_at(&rope, &spans, "Title"), Some("markup.heading"));
    assert_eq!(name_at(&rope, &spans, "emphasis"), Some("markup.italic"));
    assert_eq!(name_at(&rope, &spans, "code`"), Some("markup.raw"));
    assert_eq!(name_at(&rope, &spans, "fn main"), Some("keyword"));
    assert_eq!(name_at(&rope, &spans, "1;"), Some("constant.builtin"));
}

#[test]
fn rust_on_its_own() {
    let (rope, spans) = highlight(Lang::Rust, "// note\nfn f(a: u32) -> &str { \"s\" }\n");
    assert_eq!(name_at(&rope, &spans, "// note"), Some("comment"));
    assert_eq!(name_at(&rope, &spans, "fn"), Some("keyword"));
    assert_eq!(name_at(&rope, &spans, "\"s\""), Some("string"));
}

#[test]
fn an_edit_reparses_as_a_fresh_parse_would() {
    let mut rope = Rope::from_str("fn a() {}\nfn b() {}\n");
    let mut s = Syntax::new(Lang::Rust);
    s.parse(&rope);
    // "a" becomes "abc".
    rope.insert(4, "bc");
    s.edit(&omavim_syntax::InputEdit {
        start_byte: 4,
        old_end_byte: 4,
        new_end_byte: 6,
        start_position: omavim_syntax::Point::new(0, 4),
        old_end_position: omavim_syntax::Point::new(0, 4),
        new_end_position: omavim_syntax::Point::new(0, 6),
    });
    s.parse(&rope);
    let mut fresh = Syntax::new(Lang::Rust);
    fresh.parse(&rope);
    assert_eq!(
        s.tree().unwrap().root_node().to_sexp(),
        fresh.tree().unwrap().root_node().to_sexp()
    );
    let all = 0..rope.len_bytes();
    assert_eq!(
        s.highlights(&rope, all.clone()),
        fresh.highlights(&rope, all)
    );
}

// Timings, by hand: cargo test --release -p omavim-syntax -- --ignored --nocapture
#[test]
#[ignore]
fn timing() {
    let mut text = String::new();
    for i in 0..500 {
        text.push_str(&format!(
            "## Section {i}\n\nSome *prose* with a [link](https://x.y) and `code`, and **more** words to fill the line out.\n\n```rust\nfn f{i}(a: u32) -> u32 {{ a + {i} }}\n```\n\n"
        ));
    }
    let rope = Rope::from_str(&text);
    let mut s = Syntax::new(Lang::Markdown);
    let t = std::time::Instant::now();
    s.parse(&rope);
    eprintln!("lines {} full parse {:?}", rope.len_lines(), t.elapsed());
    let from = rope.line_to_byte(2000);
    let to = rope.line_to_byte(2200);
    let t = std::time::Instant::now();
    let n = s.highlights(&rope, from..to).len();
    eprintln!("200 lines: {n} spans in {:?}", t.elapsed());
    let t = std::time::Instant::now();
    let mut r2 = rope.clone();
    r2.insert(from, "x");
    s.edit(&omavim_syntax::InputEdit {
        start_byte: from,
        old_end_byte: from,
        new_end_byte: from + 1,
        start_position: omavim_syntax::Point::new(2000, 0),
        old_end_position: omavim_syntax::Point::new(2000, 0),
        new_end_position: omavim_syntax::Point::new(2000, 1),
    });
    s.parse(&r2);
    eprintln!("reparse after a keystroke {:?}", t.elapsed());
}

#[test]
#[ignore]
fn timing_rust() {
    let mut text = String::new();
    for i in 0..1000 {
        text.push_str(&format!(
            "/// Doc {i}\nfn f{i}(a: u32) -> u32 {{\n    a + {i}\n}}\n"
        ));
    }
    let rope = Rope::from_str(&text);
    let mut s = Syntax::new(Lang::Rust);
    let t = std::time::Instant::now();
    s.parse(&rope);
    eprintln!(
        "rust lines {} full parse {:?}",
        rope.len_lines(),
        t.elapsed()
    );
    let from = rope.line_to_byte(2000);
    let to = rope.line_to_byte(2200);
    let t = std::time::Instant::now();
    let n = s.highlights(&rope, from..to).len();
    eprintln!("rust 200 lines: {n} spans in {:?}", t.elapsed());
    let mut r2 = rope.clone();
    r2.insert(from, "x");
    let t = std::time::Instant::now();
    s.edit(&omavim_syntax::InputEdit {
        start_byte: from,
        old_end_byte: from,
        new_end_byte: from + 1,
        start_position: omavim_syntax::Point::new(2000, 0),
        old_end_position: omavim_syntax::Point::new(2000, 0),
        new_end_position: omavim_syntax::Point::new(2000, 1),
    });
    s.parse(&r2);
    eprintln!("rust reparse after a keystroke {:?}", t.elapsed());
}
