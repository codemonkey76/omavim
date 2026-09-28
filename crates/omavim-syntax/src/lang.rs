//! The bundled languages: what they're called, which files are which, and
//! their grammars and queries.

use crate::Config;
use std::path::Path;
use std::sync::OnceLock;
use tree_sitter::Query;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    Markdown,
    /// Markdown's inline parts (emphasis, links, code spans), which its
    /// grammar parses separately, inside the blocks.
    MarkdownInline,
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Tsx,
    Json,
    Toml,
    Yaml,
    Bash,
    Html,
    Css,
    Php,
    Go,
    C,
    Lua,
    Sql,
}

impl Lang {
    pub const ALL: [Lang; 18] = [
        Lang::Markdown,
        Lang::MarkdownInline,
        Lang::Rust,
        Lang::Python,
        Lang::JavaScript,
        Lang::TypeScript,
        Lang::Tsx,
        Lang::Json,
        Lang::Toml,
        Lang::Yaml,
        Lang::Bash,
        Lang::Html,
        Lang::Css,
        Lang::Php,
        Lang::Go,
        Lang::C,
        Lang::Lua,
        Lang::Sql,
    ];

    /// Its name, as `:set filetype=` takes it.
    pub fn name(self) -> &'static str {
        match self {
            Lang::Markdown => "markdown",
            Lang::MarkdownInline => "markdown_inline",
            Lang::Rust => "rust",
            Lang::Python => "python",
            Lang::JavaScript => "javascript",
            Lang::TypeScript => "typescript",
            Lang::Tsx => "tsx",
            Lang::Json => "json",
            Lang::Toml => "toml",
            Lang::Yaml => "yaml",
            Lang::Bash => "bash",
            Lang::Html => "html",
            Lang::Css => "css",
            Lang::Php => "php",
            Lang::Go => "go",
            Lang::C => "c",
            Lang::Lua => "lua",
            Lang::Sql => "sql",
        }
    }

    /// A language by name: `:set filetype=`, a fenced code block's
    /// ```` ```rust ````, or an injection query's.
    pub fn from_name(name: &str) -> Option<Lang> {
        let name = name.to_ascii_lowercase();
        Some(match name.as_str() {
            "markdown" | "md" => Lang::Markdown,
            "markdown_inline" => Lang::MarkdownInline,
            "rust" | "rs" => Lang::Rust,
            "python" | "py" | "python3" => Lang::Python,
            "javascript" | "js" | "jsx" | "node" | "mjs" | "cjs" => Lang::JavaScript,
            "typescript" | "ts" => Lang::TypeScript,
            "tsx" => Lang::Tsx,
            "json" | "jsonc" => Lang::Json,
            "toml" => Lang::Toml,
            "yaml" | "yml" => Lang::Yaml,
            "bash" | "sh" | "shell" | "zsh" | "shellscript" => Lang::Bash,
            "html" | "htm" | "xhtml" => Lang::Html,
            "css" => Lang::Css,
            "php" => Lang::Php,
            "go" | "golang" => Lang::Go,
            "c" | "h" => Lang::C,
            "lua" => Lang::Lua,
            "sql" => Lang::Sql,
            _ => return None,
        })
    }

    /// The language of a file, by its name.
    pub fn from_path(path: &Path) -> Option<Lang> {
        let file = path.file_name()?.to_str()?;
        match file {
            "PKGBUILD" | ".bashrc" | ".bash_profile" | ".profile" | ".zshrc" => {
                return Some(Lang::Bash);
            }
            "Cargo.lock" => return Some(Lang::Toml),
            _ => {}
        }
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "md" | "markdown" | "mdown" | "mkd" => Lang::Markdown,
            "rs" => Lang::Rust,
            "py" | "pyw" | "pyi" => Lang::Python,
            "js" | "mjs" | "cjs" | "jsx" => Lang::JavaScript,
            "ts" | "mts" | "cts" => Lang::TypeScript,
            "tsx" => Lang::Tsx,
            "json" | "jsonc" => Lang::Json,
            "toml" => Lang::Toml,
            "yaml" | "yml" => Lang::Yaml,
            "sh" | "bash" | "zsh" => Lang::Bash,
            "html" | "htm" | "xhtml" => Lang::Html,
            "css" => Lang::Css,
            "php" => Lang::Php,
            "go" => Lang::Go,
            "c" | "h" => Lang::C,
            "lua" => Lang::Lua,
            "sql" => Lang::Sql,
            _ => return None,
        })
    }

    /// Its grammar and queries, made the first time they're wanted.
    pub(crate) fn config(self) -> &'static Config {
        static CONFIGS: [OnceLock<Config>; 18] = [const { OnceLock::new() }; 18];
        let i = Lang::ALL.iter().position(|&l| l == self).unwrap();
        CONFIGS[i].get_or_init(|| self.load())
    }

    fn load(self) -> Config {
        let (language, highlights, injections): (tree_sitter::Language, Vec<&str>, Vec<&str>) =
            match self {
                Lang::Markdown => (
                    tree_sitter_md::LANGUAGE.into(),
                    vec![tree_sitter_md::HIGHLIGHT_QUERY_BLOCK],
                    vec![tree_sitter_md::INJECTION_QUERY_BLOCK],
                ),
                Lang::MarkdownInline => (
                    tree_sitter_md::INLINE_LANGUAGE.into(),
                    vec![tree_sitter_md::HIGHLIGHT_QUERY_INLINE],
                    vec![tree_sitter_md::INJECTION_QUERY_INLINE],
                ),
                Lang::Rust => (
                    tree_sitter_rust::LANGUAGE.into(),
                    vec![tree_sitter_rust::HIGHLIGHTS_QUERY],
                    vec![tree_sitter_rust::INJECTIONS_QUERY],
                ),
                Lang::Python => (
                    tree_sitter_python::LANGUAGE.into(),
                    vec![tree_sitter_python::HIGHLIGHTS_QUERY],
                    vec![],
                ),
                Lang::JavaScript => (
                    tree_sitter_javascript::LANGUAGE.into(),
                    vec![
                        tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
                        tree_sitter_javascript::HIGHLIGHT_QUERY,
                    ],
                    vec![tree_sitter_javascript::INJECTIONS_QUERY],
                ),
                // TypeScript's queries add to JavaScript's.
                Lang::TypeScript => (
                    tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                    vec![
                        tree_sitter_typescript::HIGHLIGHTS_QUERY,
                        tree_sitter_javascript::HIGHLIGHT_QUERY,
                    ],
                    vec![tree_sitter_javascript::INJECTIONS_QUERY],
                ),
                Lang::Tsx => (
                    tree_sitter_typescript::LANGUAGE_TSX.into(),
                    vec![
                        tree_sitter_typescript::HIGHLIGHTS_QUERY,
                        tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
                        tree_sitter_javascript::HIGHLIGHT_QUERY,
                    ],
                    vec![tree_sitter_javascript::INJECTIONS_QUERY],
                ),
                Lang::Json => (
                    tree_sitter_json::LANGUAGE.into(),
                    vec![tree_sitter_json::HIGHLIGHTS_QUERY],
                    vec![],
                ),
                Lang::Toml => (
                    tree_sitter_toml_ng::LANGUAGE.into(),
                    vec![tree_sitter_toml_ng::HIGHLIGHTS_QUERY],
                    vec![],
                ),
                Lang::Yaml => (
                    tree_sitter_yaml::LANGUAGE.into(),
                    vec![tree_sitter_yaml::HIGHLIGHTS_QUERY],
                    vec![],
                ),
                Lang::Bash => (
                    tree_sitter_bash::LANGUAGE.into(),
                    vec![tree_sitter_bash::HIGHLIGHT_QUERY],
                    vec![],
                ),
                Lang::Html => (
                    tree_sitter_html::LANGUAGE.into(),
                    vec![tree_sitter_html::HIGHLIGHTS_QUERY],
                    vec![tree_sitter_html::INJECTIONS_QUERY],
                ),
                Lang::Css => (
                    tree_sitter_css::LANGUAGE.into(),
                    vec![tree_sitter_css::HIGHLIGHTS_QUERY],
                    vec![],
                ),
                Lang::Php => (
                    tree_sitter_php::LANGUAGE_PHP.into(),
                    vec![tree_sitter_php::HIGHLIGHTS_QUERY],
                    vec![tree_sitter_php::INJECTIONS_QUERY],
                ),
                Lang::Go => (
                    tree_sitter_go::LANGUAGE.into(),
                    vec![tree_sitter_go::HIGHLIGHTS_QUERY],
                    vec![],
                ),
                Lang::C => (
                    tree_sitter_c::LANGUAGE.into(),
                    vec![tree_sitter_c::HIGHLIGHT_QUERY],
                    vec![],
                ),
                Lang::Lua => (
                    tree_sitter_lua::LANGUAGE.into(),
                    vec![tree_sitter_lua::HIGHLIGHTS_QUERY],
                    vec![tree_sitter_lua::INJECTIONS_QUERY],
                ),
                Lang::Sql => (
                    tree_sitter_sequel::LANGUAGE.into(),
                    vec![tree_sitter_sequel::HIGHLIGHTS_QUERY],
                    vec![],
                ),
            };
        let highlights = query(&language, &highlights.concat())
            .unwrap_or_else(|e| panic!("{}'s highlights query: {e}", self.name()));
        let injections = if injections.is_empty() {
            None
        } else {
            Some(
                query(&language, &injections.concat())
                    .unwrap_or_else(|e| panic!("{}'s injections query: {e}", self.name())),
            )
        };
        let highlight_names = highlights
            .capture_names()
            .iter()
            .map(|n| highlight_name(n))
            .collect();
        let index =
            |q: &Option<Query>, name: &str| q.as_ref().and_then(|q| q.capture_index_for_name(name));
        let textobjects = self.textobjects_source().map(|source| {
            query(&language, source)
                .unwrap_or_else(|e| panic!("{}'s textobjects query: {e}", self.name()))
        });
        Config {
            textobjects,
            injection_language: index(&injections, "injection.language"),
            injection_content: index(&injections, "injection.content"),
            language,
            highlights,
            injections,
            highlight_names,
        }
    }
}

impl Lang {
    /// Its text objects query (vendored from nvim-treesitter-textobjects by
    /// tools/syntax/vendor-textobjects.py).
    fn textobjects_source(self) -> Option<&'static str> {
        macro_rules! q {
            ($l:literal) => {
                include_str!(concat!("../queries/", $l, "/textobjects.scm"))
            };
        }
        Some(match self {
            Lang::Rust => q!("rust"),
            Lang::Python => q!("python"),
            Lang::JavaScript => q!("javascript"),
            Lang::TypeScript => q!("typescript"),
            Lang::Tsx => q!("tsx"),
            Lang::Json => q!("json"),
            Lang::Toml => q!("toml"),
            Lang::Yaml => q!("yaml"),
            Lang::Bash => q!("bash"),
            Lang::Html => q!("html"),
            Lang::Css => q!("css"),
            Lang::Php => q!("php"),
            Lang::Go => q!("go"),
            Lang::C => q!("c"),
            Lang::Lua => q!("lua"),
            Lang::Markdown | Lang::MarkdownInline | Lang::Sql => return None,
        })
    }
}

fn query(language: &tree_sitter::Language, source: &str) -> Result<Query, tree_sitter::QueryError> {
    Query::new(language, source)
}

/// A capture's highlight name, as Helix names it (the Markdown grammar's
/// queries use nvim-treesitter's older names). None for ones that aren't
/// highlights (`@none`, `@_private`).
fn highlight_name(capture: &str) -> Option<&'static str> {
    let name = match capture {
        "none" => return None,
        c if c.starts_with('_') || c.starts_with("injection") || c.starts_with("local") => {
            return None;
        }
        "text.title" => "markup.heading",
        "text.emphasis" => "markup.italic",
        "text.strong" => "markup.bold",
        "text.literal" => "markup.raw",
        "text.uri" => "markup.link.url",
        "text.reference" => "markup.link.text",
        other => other,
    };
    Some(Box::leak(name.to_string().into_boxed_str()))
}
