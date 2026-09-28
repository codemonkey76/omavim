//! Tree-sitter for Omavim: which language a file is, parsing it (again,
//! incrementally, after each edit), and the highlight names for what's on
//! screen, with code blocks in Markdown (and the like) highlighted in their
//! own language.
//!
//! The grammars are compiled in, one crate each, with the highlight and
//! injection queries they ship (see NOTICE.md).

mod lang;
mod objects;

pub use lang::Lang;
pub use tree_sitter::{InputEdit, Point};

use ropey::Rope;
use std::ops::Range;
use tree_sitter::{Node, Parser, Query, QueryCursor, StreamingIterator, TextProvider, Tree};

/// A highlighted stretch of the text: bytes, and a highlight name
/// (`keyword`, `markup.heading`, ... as Helix names them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub range: Range<usize>,
    pub name: &'static str,
}

/// The syntax tree of a document, kept up to date as it's edited.
pub struct Syntax {
    lang: Lang,
    parser: Parser,
    tree: Option<Tree>,
}

impl Syntax {
    pub fn new(lang: Lang) -> Self {
        let mut parser = Parser::new();
        parser
            .set_language(&lang.config().language)
            .expect("a bundled grammar loads");
        Self {
            lang,
            parser,
            tree: None,
        }
    }

    pub fn lang(&self) -> Lang {
        self.lang
    }

    pub fn tree(&self) -> Option<&Tree> {
        self.tree.as_ref()
    }

    /// The text changed: tell the old tree where, so the next parse only
    /// redoes what's affected.
    pub fn edit(&mut self, edit: &InputEdit) {
        if let Some(tree) = &mut self.tree {
            tree.edit(edit);
        }
    }

    /// Every `kind` of text object (`function`, `class`, `parameter`,
    /// `comment`; in Markdown `emphasis`, `link`, `heading`, `code`), or
    /// just the inside of each: byte ranges. Those in code embedded in the
    /// text too, where it's embedded around `near` (a byte).
    pub fn objects(&self, text: &Rope, kind: &str, inner: bool, near: usize) -> Vec<Range<usize>> {
        let Some(tree) = &self.tree else {
            return Vec::new();
        };
        let mut out = Vec::new();
        objects::collect(
            self.lang,
            tree.root_node(),
            text,
            kind,
            inner,
            near,
            0,
            &mut out,
        );
        out
    }

    /// Parse the text (again, after edits).
    pub fn parse(&mut self, text: &Rope) {
        self.tree = parse_rope(&mut self.parser, text, self.tree.as_ref());
    }

    /// The highlights over `range` (bytes): sorted, not overlapping. Code
    /// the document embeds (a fenced block, a `<script>`) is highlighted in
    /// its own language.
    pub fn highlights(&self, text: &Rope, range: Range<usize>) -> Vec<Span> {
        let Some(tree) = &self.tree else {
            return Vec::new();
        };
        let range = range.start.min(text.len_bytes())..range.end.min(text.len_bytes());
        let mut paint: Vec<Option<&'static str>> = vec![None; range.len()];
        highlight_layer(self.lang, tree.root_node(), text, &range, &mut paint, 0);
        spans(&paint, range.start)
    }
}

fn parse_rope(parser: &mut Parser, text: &Rope, old: Option<&Tree>) -> Option<Tree> {
    let len = text.len_bytes();
    parser.parse_with_options(
        &mut |byte: usize, _: Point| -> &[u8] {
            if byte >= len {
                return &[];
            }
            let (chunk, start, _, _) = text.chunk_at_byte(byte);
            &chunk.as_bytes()[byte - start..]
        },
        old,
        None,
    )
}

/// The text of a node, for queries' `#eq?` and `#match?`.
pub(crate) struct RopeText<'a>(&'a Rope);

impl<'a> TextProvider<&'a [u8]> for RopeText<'a> {
    type I = std::vec::IntoIter<&'a [u8]>;

    fn text(&mut self, node: Node) -> Self::I {
        let rope = self.0;
        let r = node.byte_range();
        let r = r.start.min(rope.len_bytes())..r.end.min(rope.len_bytes());
        rope.byte_slice(r)
            .chunks()
            .map(str::as_bytes)
            .collect::<Vec<_>>()
            .into_iter()
    }
}

/// How deep injections go (Markdown, its code block, a string in that...).
pub(crate) const MAX_DEPTH: usize = 3;

/// Paint `paint` (a name per byte of `range`) with one layer's highlights,
/// then with its injections' over them.
fn highlight_layer(
    lang: Lang,
    root: Node,
    text: &Rope,
    range: &Range<usize>,
    paint: &mut [Option<&'static str>],
    depth: usize,
) {
    let config = lang.config();
    let mut cursor = QueryCursor::new();
    cursor.set_byte_range(range.clone());
    // Outer nodes first, and for the same node the query's first pattern
    // last, so it wins (as tree-sitter's highlighter does).
    let mut found: Vec<(Range<usize>, usize, Option<&'static str>)> = Vec::new();
    let mut matches = cursor.matches(&config.highlights, root, RopeText(text));
    while let Some(m) = matches.next() {
        for c in m.captures {
            let name = config.highlight_names[c.index as usize];
            found.push((c.node.byte_range(), m.pattern_index, name));
        }
    }
    found.sort_by(|a, b| {
        (b.0.end - b.0.start)
            .cmp(&(a.0.end - a.0.start))
            .then(b.1.cmp(&a.1))
    });
    for (r, _, name) in found {
        let from = r.start.max(range.start) - range.start;
        let to = r.end.min(range.end).saturating_sub(range.start);
        if from < to {
            paint[from..to].fill(name);
        }
    }
    if depth >= MAX_DEPTH {
        return;
    }
    for (inner, ranges) in injections(lang, root, text, range) {
        let Some(tree) = parse_injection(inner, &ranges, text) else {
            continue;
        };
        // The embedded language's own colours go over the outer ones.
        highlight_layer(inner, tree.root_node(), text, range, paint, depth + 1);
    }
}

/// The code a layer embeds over `range`: each embedded language, and where
/// its text is.
pub(crate) fn injections(
    lang: Lang,
    root: Node,
    text: &Rope,
    range: &Range<usize>,
) -> Vec<(Lang, Vec<tree_sitter::Range>)> {
    let config = lang.config();
    let Some(injections) = &config.injections else {
        return Vec::new();
    };
    let mut cursor = QueryCursor::new();
    cursor.set_byte_range(range.clone());
    let mut matches = cursor.matches(injections, root, RopeText(text));
    let mut layers: Vec<(Lang, Vec<tree_sitter::Range>)> = Vec::new();
    while let Some(m) = matches.next() {
        let mut lang_name: Option<String> = None;
        let mut content: Option<Node> = None;
        for c in m.captures {
            if Some(c.index) == config.injection_language {
                lang_name = Some(text.byte_slice(c.node.byte_range()).to_string());
            } else if Some(c.index) == config.injection_content {
                content = Some(c.node);
            }
        }
        let settings = injections.property_settings(m.pattern_index);
        let include_children = settings
            .iter()
            .any(|p| &*p.key == "injection.include-children");
        for p in settings {
            if &*p.key == "injection.language" {
                lang_name = p.value.as_deref().map(str::to_string);
            }
        }
        let (Some(name), Some(node)) = (lang_name, content) else {
            continue;
        };
        let Some(inner) = Lang::from_name(name.trim()) else {
            continue;
        };
        layers.push((inner, content_ranges(node, include_children)));
    }
    layers
}

/// Parse embedded code: just its parts of the text.
pub(crate) fn parse_injection(
    lang: Lang,
    ranges: &[tree_sitter::Range],
    text: &Rope,
) -> Option<Tree> {
    if ranges.is_empty() {
        return None;
    }
    let mut parser = Parser::new();
    parser.set_language(&lang.config().language).ok()?;
    parser.set_included_ranges(ranges).ok()?;
    parse_rope(&mut parser, text, None)
}

/// The ranges of an injection's content: the node, less its named
/// children's (a block quote's `>` in a code block), unless they're
/// included.
fn content_ranges(node: Node, include_children: bool) -> Vec<tree_sitter::Range> {
    let whole = node.range();
    if include_children {
        return vec![whole];
    }
    let mut out = Vec::new();
    let mut start = (whole.start_byte, whole.start_point);
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        let r = child.range();
        if r.start_byte > start.0 {
            out.push(tree_sitter::Range {
                start_byte: start.0,
                end_byte: r.start_byte,
                start_point: start.1,
                end_point: r.start_point,
            });
        }
        start = (r.end_byte, r.end_point);
    }
    if whole.end_byte > start.0 {
        out.push(tree_sitter::Range {
            start_byte: start.0,
            end_byte: whole.end_byte,
            start_point: start.1,
            end_point: whole.end_point,
        });
    }
    out
}

/// A name per byte into spans.
fn spans(paint: &[Option<&'static str>], base: usize) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    for (i, name) in paint.iter().enumerate() {
        let Some(name) = *name else { continue };
        match out.last_mut() {
            Some(s) if s.name == name && s.range.end == base + i => s.range.end += 1,
            _ => out.push(Span {
                range: base + i..base + i + 1,
                name,
            }),
        }
    }
    out
}

/// A language's grammar and queries, ready to use.
pub(crate) struct Config {
    pub language: tree_sitter::Language,
    pub highlights: Query,
    pub injections: Option<Query>,
    /// The highlight name for each of the highlight query's captures (None
    /// for `@none` and the like).
    pub highlight_names: Vec<Option<&'static str>>,
    pub injection_language: Option<u32>,
    pub injection_content: Option<u32>,
    /// Where functions, classes, parameters and comments are (from
    /// nvim-treesitter-textobjects).
    pub textobjects: Option<Query>,
}
