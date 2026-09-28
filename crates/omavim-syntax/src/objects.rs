//! Text objects from the syntax tree: code's from nvim-treesitter-textobjects'
//! queries, Markdown's from its nodes.

use crate::{Lang, MAX_DEPTH, RopeText, injections, parse_injection};
use ropey::Rope;
use std::ops::Range;
use tree_sitter::{Node, QueryCursor, QueryPredicateArg, StreamingIterator};

/// Add a layer's objects to `out`, then those of the code it embeds around
/// `near`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn collect(
    lang: Lang,
    root: Node,
    text: &Rope,
    kind: &str,
    inner: bool,
    near: usize,
    depth: usize,
    out: &mut Vec<Range<usize>>,
) {
    match lang {
        Lang::Markdown | Lang::MarkdownInline => markdown(root, text, kind, inner, out),
        _ => query(lang, root, text, kind, inner, out),
    }
    if depth >= MAX_DEPTH {
        return;
    }
    let near = near.min(text.len_bytes());
    for (embedded, ranges) in injections(lang, root, text, &(near..near + 1)) {
        let inside = ranges
            .iter()
            .any(|r| r.start_byte <= near && near <= r.end_byte);
        if !inside {
            continue;
        }
        if let Some(tree) = parse_injection(embedded, &ranges, text) {
            collect(
                embedded,
                tree.root_node(),
                text,
                kind,
                inner,
                near,
                depth + 1,
                out,
            );
        }
    }
}

/// From the language's textobjects query: each match's `@kind.inner` (or
/// `.outer`), all of its nodes together, moved by any `#offset!`.
fn query(
    lang: Lang,
    root: Node,
    text: &Rope,
    kind: &str,
    inner: bool,
    out: &mut Vec<Range<usize>>,
) {
    let Some(q) = &lang.config().textobjects else {
        return;
    };
    let name = format!("{kind}.{}", if inner { "inner" } else { "outer" });
    let Some(want) = q.capture_index_for_name(&name) else {
        return;
    };
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(q, root, RopeText(text));
    while let Some(m) = matches.next() {
        let mut range: Option<Range<usize>> = None;
        for c in m.captures.iter().filter(|c| c.index == want) {
            let r = c.node.byte_range();
            range = Some(match range {
                Some(a) => a.start.min(r.start)..a.end.max(r.end),
                None => r,
            });
        }
        let Some(mut range) = range else { continue };
        for p in q.general_predicates(m.pattern_index) {
            if &*p.operator != "offset!" {
                continue;
            }
            let [
                QueryPredicateArg::Capture(i),
                _,
                QueryPredicateArg::String(sc),
                _,
                QueryPredicateArg::String(ec),
            ] = &p.args[..]
            else {
                continue;
            };
            if *i != want {
                continue;
            }
            let (sc, ec): (isize, isize) = (sc.parse().unwrap_or(0), ec.parse().unwrap_or(0));
            let start = range.start.saturating_add_signed(sc);
            let end = range.end.saturating_add_signed(ec);
            range = start..end.max(start);
        }
        out.push(range);
    }
}

/// Markdown's, by node: emphasis (and strikethrough and code spans) inside
/// the marks, links (the text inside), heading sections (the section below
/// the heading inside) and code blocks (the code inside).
fn markdown(root: Node, text: &Rope, kind: &str, inner: bool, out: &mut Vec<Range<usize>>) {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        let mut cursor = node.walk();
        stack.extend(node.children(&mut cursor));
        let whole = node.byte_range();
        let found = match (kind, node.kind()) {
            ("emphasis", "emphasis" | "strong_emphasis") | ("emphasis", "strikethrough")
                if node.parent().is_none_or(|p| p.kind() != "strikethrough") =>
            {
                Some(if inner {
                    trim_marks(text, whole, &['*', '_', '~'])
                } else {
                    whole
                })
            }
            ("code", "code_span") => Some(if inner {
                trim_marks(text, whole, &['`'])
            } else {
                whole
            }),
            (
                "link",
                "inline_link"
                | "full_reference_link"
                | "collapsed_reference_link"
                | "shortcut_link"
                | "image",
            ) => Some(if inner {
                child(node, &["link_text", "image_description"]).unwrap_or(whole)
            } else {
                whole
            }),
            ("link", "uri_autolink" | "email_autolink") => Some(if inner {
                whole.start + 1..whole.end - 1
            } else {
                whole
            }),
            ("heading", "section") => Some(if inner {
                match node.child(0) {
                    // From the first line of text after the heading.
                    Some(h) if h.kind().ends_with("heading") => {
                        let mut start = h.end_byte();
                        while start < whole.end && text.byte(start) == b'\n' {
                            start += 1;
                        }
                        start..whole.end
                    }
                    _ => whole,
                }
            } else {
                whole
            }),
            ("code", "fenced_code_block") => {
                if inner {
                    child(node, &["code_fence_content"])
                } else {
                    Some(whole)
                }
            }
            ("code", "indented_code_block") => Some(whole),
            _ => None,
        };
        if let Some(r) = found {
            out.push(r);
        }
    }
}

/// A child of one of these kinds.
fn child(node: Node, kinds: &[&str]) -> Option<Range<usize>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|c| kinds.contains(&c.kind()))
        .map(|c| c.byte_range())
}

/// Inside the marks around some text: as many off each end as match.
fn trim_marks(text: &Rope, r: Range<usize>, marks: &[char]) -> Range<usize> {
    let s = text.byte_slice(r.clone()).to_string();
    let lead = s.chars().take_while(|c| marks.contains(c)).count();
    let trail = s.chars().rev().take_while(|c| marks.contains(c)).count();
    let n = lead.min(trail);
    if 2 * n >= s.len() {
        return r;
    }
    r.start + n..r.end - n
}
