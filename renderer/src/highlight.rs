// VS Code-style syntax highlighting (Layer 1): TextMate-family grammars run by
// `syntect` (pure-Rust fancy-regex backend), with scopes mapped to Nova's theme
// colors. This replaces the toy single-line TextMate interpreter + the
// tree-sitter JSON/Rust path: syntect bundles JS/TS/JSON/CSS/HTML/Python/Rust/…
// so common languages color out of the box.
//
// Tokenization is stateful per line (a `ScopeStack` carried across lines), which
// is exactly what makes incremental re-highlighting possible: `LineCache` stores
// the parse state at each line boundary so an edit only re-tokenizes from the
// changed line until the carried state reconverges.

use std::sync::OnceLock;

use glyphon::{Attrs, Color, Family};
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};

fn syntax_set() -> &'static SyntaxSet {
    static S: OnceLock<SyntaxSet> = OnceLock::new();
    S.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// The bundled syntax for a file extension, if any.
fn syntax_for(ext: &str) -> Option<&'static SyntaxReference> {
    let ss = syntax_set();
    ss.find_syntax_by_extension(ext)
        .or_else(|| ss.find_syntax_by_token(ext))
}

/// True if we have a grammar for this extension (so callers can skip the fallback).
pub fn supports(ext: &str) -> bool {
    syntax_for(ext).is_some()
}

/// Map a TextMate/Sublime scope string to a Nova theme color by its leading
/// standard segment (ported from the old textmate interpreter). The token's
/// deepest scope wins.
pub fn scope_color(s: &str) -> Color {
    use crate::theme;
    if s.starts_with("comment") {
        theme::SYN_COMMENT()
    } else if s.starts_with("string") || s.starts_with("constant.character") {
        theme::SYN_STRING()
    } else if s.starts_with("keyword.control") {
        theme::SYN_KEYWORD_CTRL()
    } else if s.starts_with("keyword") || s.starts_with("storage") {
        theme::SYN_KEYWORD()
    } else if s.contains("entity.name.function") || s.contains("support.function") || s.contains("meta.function-call") {
        theme::SYN_FUNCTION()
    } else if s.contains("entity.name.type")
        || s.contains("support.type")
        || s.contains("entity.name.class")
        || s.contains("entity.other.inherited-class")
    {
        theme::SYN_TYPE()
    } else if s.starts_with("constant.numeric") {
        theme::SYN_NUMBER()
    } else if s.starts_with("constant") || s.starts_with("support.constant") {
        theme::SYN_CONSTANT()
    } else if s.starts_with("variable") || s.starts_with("entity.name") {
        theme::SYN_VARIABLE()
    } else if s.starts_with("invalid") {
        Color::rgb(0xF4, 0x47, 0x47)
    } else {
        theme::FG_TEXT()
    }
}

fn attrs(color: Color) -> Attrs<'static> {
    Attrs::new().family(Family::Name(crate::theme::MONO_FAMILY())).color(color)
}

/// The color for the top of a scope stack (deepest scope drives the color).
fn color_for_stack(stack: &ScopeStack) -> Color {
    match stack.as_slice().last() {
        Some(scope) => scope_color(&scope_string(*scope)),
        None => crate::theme::FG_TEXT(),
    }
}

/// Build the dotted string for a syntect `Scope` (via the global scope repo).
fn scope_string(scope: Scope) -> String {
    scope.build_string()
}

/// Per-document incremental tokenizer state: the parse state + scope stack at the
/// START of each line, so editing line N only re-tokenizes from N forward until
/// the carried state matches the cached state (reconvergence).
pub struct LineCache {
    ext: String,
    /// (ParseState, ScopeStack) snapshot at the start of line `i`.
    starts: Vec<(ParseState, ScopeStack)>,
    /// Cached rich-text spans (substring, attrs) for line `i`, including its `\n`.
    spans: Vec<Vec<(String, Attrs<'static>)>>,
}

impl LineCache {
    pub fn new(ext: &str) -> Option<LineCache> {
        let syntax = syntax_for(ext)?;
        Some(LineCache {
            ext: ext.to_string(),
            starts: vec![(ParseState::new(syntax), ScopeStack::new())],
            spans: Vec::new(),
        })
    }

    /// Re-tokenize `text` from `dirty_line` onward, reusing cached line states
    /// before it. Returns the full document's rich-text spans (concatenated lines).
    /// Pass `dirty_line = 0` for a fresh document.
    pub fn highlight(&mut self, text: &str, dirty_line: usize) -> Vec<(String, Attrs<'static>)> {
        let ss = syntax_set();
        let lines: Vec<&str> = LinesWithEndingsIter::new(text).collect();
        // Truncate caches to the dirty boundary (keep states for [0, dirty_line]).
        let start = dirty_line.min(self.starts.len().saturating_sub(1));
        self.starts.truncate(start + 1);
        self.spans.truncate(start);

        for i in start..lines.len() {
            let (mut state, mut stack) = self.starts[i].clone();
            let line = lines[i];
            let mut line_spans: Vec<(String, Attrs<'static>)> = Vec::new();
            if let Ok(ops) = state.parse_line(line, ss) {
                let mut last = 0usize;
                for (idx, op) in ops {
                    if idx > last {
                        line_spans.push((line[last..idx].to_string(), attrs(color_for_stack(&stack))));
                    }
                    stack.apply(&op).ok();
                    last = idx;
                }
                if last < line.len() {
                    line_spans.push((line[last..].to_string(), attrs(color_for_stack(&stack))));
                }
            } else {
                line_spans.push((line.to_string(), attrs(crate::theme::FG_TEXT())));
            }
            if i < self.spans.len() {
                self.spans[i] = line_spans;
            } else {
                self.spans.push(line_spans);
            }
            // Snapshot the state at the start of the NEXT line.
            let next = (state, stack);
            if i + 1 < self.starts.len() {
                self.starts[i + 1] = next;
            } else {
                self.starts.push(next);
            }
        }
        // Drop any trailing caches if the document shrank.
        self.spans.truncate(lines.len());
        self.starts.truncate(lines.len() + 1);

        self.spans.iter().flatten().cloned().collect()
    }

    pub fn ext(&self) -> &str {
        &self.ext
    }
}

/// Iterate lines keeping their trailing `\n` (syntect tokenizes with line endings).
struct LinesWithEndingsIter<'a> {
    text: &'a str,
    pos: usize,
}
impl<'a> LinesWithEndingsIter<'a> {
    fn new(text: &'a str) -> Self {
        Self { text, pos: 0 }
    }
}
impl<'a> Iterator for LinesWithEndingsIter<'a> {
    type Item = &'a str;
    fn next(&mut self) -> Option<&'a str> {
        if self.pos >= self.text.len() {
            return None;
        }
        let rest = &self.text[self.pos..];
        let end = rest.find('\n').map(|i| i + 1).unwrap_or(rest.len());
        let line = &rest[..end];
        self.pos += end;
        Some(line)
    }
}
