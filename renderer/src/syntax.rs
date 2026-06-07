// Tree-sitter syntax highlighting. Produces (text, Attrs) spans for a document's
// glyphon buffer, coloured per VSCode Dark+. Configurations are built once and
// cached; unsupported languages return None (caller falls back to plain text).

use std::sync::OnceLock;

use glyphon::{Attrs, Color, Family};
use tree_sitter_highlight::{Highlight, HighlightConfiguration, HighlightEvent, Highlighter};

use crate::theme;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Rust,
    Json,
    Markdown,
    TypeScript,
    Tsx,
    PlainText,
}

impl Lang {
    pub fn from_ext(ext: &str) -> Lang {
        match ext.to_ascii_lowercase().as_str() {
            "rs" => Lang::Rust,
            "json" | "mcp" => Lang::Json,
            "md" | "markdown" => Lang::Markdown,
            // The TypeScript grammar is a superset of JS, so plain JS files parse
            // cleanly under it; TSX/JSX use the JSX-aware variant.
            "ts" | "mts" | "cts" | "js" | "mjs" | "cjs" => Lang::TypeScript,
            "tsx" | "jsx" => Lang::Tsx,
            _ => Lang::PlainText,
        }
    }
}

/// Capture names we recognise. The `Highlight(usize)` index returned by the
/// highlighter indexes into this list.
const HIGHLIGHT_NAMES: &[&str] = &[
    "attribute",
    "comment",
    "constant",
    "constant.builtin",
    "constructor",
    "escape",
    "function",
    "function.builtin",
    "function.macro",
    "function.method",
    "keyword",
    "keyword.control",
    "label",
    "number",
    "operator",
    "property",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "string",
    "string.escape",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
];

fn color_for(idx: usize) -> Color {
    match HIGHLIGHT_NAMES[idx] {
        "comment" => theme::SYN_COMMENT(),
        "string" | "string.escape" | "escape" => theme::SYN_STRING(),
        "keyword.control" => theme::SYN_KEYWORD_CTRL(),
        "keyword" | "operator" => theme::SYN_KEYWORD(),
        "function" | "function.builtin" | "function.macro" | "function.method" | "attribute" => {
            theme::SYN_FUNCTION()
        }
        "type" | "type.builtin" | "constructor" => theme::SYN_TYPE(),
        "number" => theme::SYN_NUMBER(),
        "constant" | "constant.builtin" | "variable.builtin" => theme::SYN_CONSTANT(),
        "property" | "variable.parameter" => theme::SYN_VARIABLE(),
        "label" => theme::SYN_LABEL(),
        _ => theme::FG_TEXT(),
    }
}

/// The TypeScript highlights query is only the TS-specific delta; it must be
/// layered on top of the JavaScript base query (same approach as Helix/nvim), or
/// core tokens — keywords, comments, strings, calls — go uncolored. Built once.
fn ts_highlights_query() -> &'static str {
    static Q: OnceLock<String> = OnceLock::new();
    Q.get_or_init(|| {
        format!(
            "{}\n{}",
            tree_sitter_javascript::HIGHLIGHT_QUERY,
            tree_sitter_typescript::HIGHLIGHTS_QUERY
        )
    })
}

fn config_for(lang: Lang) -> Option<&'static HighlightConfiguration> {
    match lang {
        Lang::Rust => {
            static C: OnceLock<HighlightConfiguration> = OnceLock::new();
            Some(C.get_or_init(|| {
                let mut c = HighlightConfiguration::new(
                    tree_sitter_rust::LANGUAGE.into(),
                    "rust",
                    tree_sitter_rust::HIGHLIGHTS_QUERY,
                    "",
                    "",
                )
                .expect("rust highlight config");
                c.configure(HIGHLIGHT_NAMES);
                c
            }))
        }
        Lang::Json => {
            static C: OnceLock<HighlightConfiguration> = OnceLock::new();
            Some(C.get_or_init(|| {
                let mut c = HighlightConfiguration::new(
                    tree_sitter_json::LANGUAGE.into(),
                    "json",
                    tree_sitter_json::HIGHLIGHTS_QUERY,
                    "",
                    "",
                )
                .expect("json highlight config");
                c.configure(HIGHLIGHT_NAMES);
                c
            }))
        }
        Lang::TypeScript => {
            static C: OnceLock<HighlightConfiguration> = OnceLock::new();
            Some(C.get_or_init(|| {
                let mut c = HighlightConfiguration::new(
                    tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                    "typescript",
                    &ts_highlights_query(),
                    "",
                    tree_sitter_typescript::LOCALS_QUERY,
                )
                .expect("typescript highlight config");
                c.configure(HIGHLIGHT_NAMES);
                c
            }))
        }
        Lang::Tsx => {
            static C: OnceLock<HighlightConfiguration> = OnceLock::new();
            Some(C.get_or_init(|| {
                let mut c = HighlightConfiguration::new(
                    tree_sitter_typescript::LANGUAGE_TSX.into(),
                    "tsx",
                    &ts_highlights_query(),
                    "",
                    tree_sitter_typescript::LOCALS_QUERY,
                )
                .expect("tsx highlight config");
                c.configure(HIGHLIGHT_NAMES);
                c
            }))
        }
        _ => None,
    }
}

/// Tree-sitter highlight `text` for `lang`, returning per-LINE colored spans
/// `(substring, color)` — the shape the incremental `LineCache` consumes. A
/// trailing '\n' produces a span ending the line (cosmic-text counts no extra
/// empty line). `None` for languages without a tree-sitter config.
pub fn highlight_lines(lang: Lang, text: &str) -> Option<Vec<Vec<(String, Color)>>> {
    let config = config_for(lang)?;
    let mut hl = Highlighter::new();
    let events = hl.highlight(config, text.as_bytes(), None, |_| None).ok()?;
    let mut lines: Vec<Vec<(String, Color)>> = vec![Vec::new()];
    let mut stack: Vec<usize> = Vec::new();
    for ev in events {
        match ev.ok()? {
            HighlightEvent::HighlightStart(Highlight(i)) => stack.push(i),
            HighlightEvent::HighlightEnd => {
                stack.pop();
            }
            HighlightEvent::Source { start, end } => {
                if start >= end {
                    continue;
                }
                let color = stack.last().map(|&i| color_for(i)).unwrap_or_else(theme::FG_TEXT);
                // Split the source slice across line boundaries so each output row
                // holds only its own text (including the trailing '\n').
                let chunk = &text[start..end];
                let mut rest = chunk;
                while let Some(nl) = rest.find('\n') {
                    let (head, tail) = rest.split_at(nl + 1);
                    lines.last_mut().unwrap().push((head.to_string(), color));
                    lines.push(Vec::new());
                    rest = tail;
                }
                if !rest.is_empty() {
                    lines.last_mut().unwrap().push((rest.to_string(), color));
                }
            }
        }
    }
    // A trailing '\n' pushed an empty final row; cosmic-text doesn't count it.
    if lines.len() > 1 && lines.last().map_or(false, |l| l.is_empty()) {
        lines.pop();
    }
    Some(lines)
}

/// Highlight `text` for `lang`, returning per-span (text, attrs). Returns None
/// for languages without a tree-sitter config (caller uses plain text).
pub fn highlight_spans(lang: Lang, text: &str) -> Option<Vec<(String, Attrs<'static>)>> {
    let config = config_for(lang)?;
    let mono = |c: Color| Attrs::new().family(Family::Name(theme::MONO_FAMILY())).color(c);
    let mut hl = Highlighter::new();
    let events = hl.highlight(config, text.as_bytes(), None, |_| None).ok()?;
    let mut spans: Vec<(String, Attrs<'static>)> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for ev in events {
        match ev.ok()? {
            HighlightEvent::HighlightStart(Highlight(i)) => stack.push(i),
            HighlightEvent::HighlightEnd => {
                stack.pop();
            }
            HighlightEvent::Source { start, end } => {
                if start >= end {
                    continue;
                }
                let color = stack.last().map(|&i| color_for(i)).unwrap_or(theme::FG_TEXT());
                spans.push((text[start..end].to_string(), mono(color)));
            }
        }
    }
    Some(spans)
}
