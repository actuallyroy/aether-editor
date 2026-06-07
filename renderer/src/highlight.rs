// VS Code-style syntax highlighting (Layer 1): TextMate-family grammars run by
// `syntect` (pure-Rust fancy-regex backend), with scopes mapped to Aether's theme
// colors. This replaces the toy single-line TextMate interpreter + the
// tree-sitter JSON/Rust path: syntect bundles JS/TS/JSON/CSS/HTML/Python/Rust/…
// so common languages color out of the box.
//
// Tokenization is stateful per line (a `ScopeStack` carried across lines), which
// is exactly what makes incremental re-highlighting possible: `LineCache` stores
// the parse state at each line boundary so an edit only re-tokenizes from the
// changed line until the carried state reconverges.

use std::sync::OnceLock;

use glyphon::Color;
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};

fn syntax_set() -> &'static SyntaxSet {
    static S: OnceLock<SyntaxSet> = OnceLock::new();
    S.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// The bundled syntax for a file extension, if any. syntect's default set bundles
/// JavaScript but not TypeScript, so TS-family extensions fall back to the JS grammar
/// (a near-superset); Layer 2 semantic tokens fill in the type-specific coloring.
fn syntax_for(ext: &str) -> Option<&'static SyntaxReference> {
    let ss = syntax_set();
    ss.find_syntax_by_extension(ext)
        .or_else(|| ss.find_syntax_by_token(ext))
        .or_else(|| match ext {
            "ts" | "mts" | "cts" | "tsx" | "jsx" | "mjs" | "cjs" => ss.find_syntax_by_extension("js"),
            _ => None,
        })
}

/// True if we have a grammar for this extension (so callers can skip the fallback).
pub fn supports(ext: &str) -> bool {
    syntax_for(ext).is_some()
}

/// Map a TextMate/Sublime scope string to a Aether theme color by its leading
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

/// Color for an LSP semantic token type (Layer 2). `None` = don't override the
/// Layer-1 color (so we only recolor where semantic info is meaningful).
pub fn semantic_color(token_type: &str) -> Option<Color> {
    use crate::theme;
    Some(match token_type {
        "namespace" | "type" | "class" | "enum" | "interface" | "struct" | "typeParameter" | "decorator" => {
            theme::SYN_TYPE()
        }
        "function" | "method" | "macro" => theme::SYN_FUNCTION(),
        "parameter" => theme::SYN_NUMBER(), // distinct hue (params can't be told apart by TextMate)
        "variable" | "property" | "enumMember" | "event" => theme::SYN_VARIABLE(),
        "keyword" | "modifier" => theme::SYN_KEYWORD(),
        "string" => theme::SYN_STRING(),
        "number" => theme::SYN_NUMBER(),
        "comment" => theme::SYN_COMMENT(),
        _ => return None,
    })
}

/// Decode an LSP semantic-tokens `data` array (groups of 5 u32: deltaLine,
/// deltaStartChar, length, tokenType, tokenModifiers) against the server's `legend`
/// (token-type names) into absolute `(line, start_utf16, len_utf16, color)` tokens.
/// Tokens whose type has no distinct color are dropped (Layer 1 shows through).
pub fn decode_semantic(data: &[u32], legend: &[String]) -> Vec<(u32, u32, u32, Color)> {
    let mut out = Vec::new();
    let (mut line, mut start) = (0u32, 0u32);
    for chunk in data.chunks_exact(5) {
        let (dl, ds, len, ttype) = (chunk[0], chunk[1], chunk[2], chunk[3] as usize);
        if dl > 0 {
            line += dl;
            start = ds;
        } else {
            start += ds;
        }
        if let Some(color) = legend.get(ttype).and_then(|t| semantic_color(t)) {
            out.push((line, start, len, color));
        }
    }
    out
}

/// The color for the top of a scope stack (deepest scope drives the color) —
/// with one whole-stack exception: a string inside a mapping KEY context (JSON
/// object keys, YAML keys) colors as a property name, not a string. Without
/// this, an entire JSON document reads as one uniform string color (#37).
fn color_for_stack(stack: &ScopeStack) -> Color {
    let scopes = stack.as_slice();
    if scopes.iter().any(|s| {
        let st = scope_string(*s);
        st.starts_with("meta.mapping.key") || st.starts_with("support.type.property-name")
    }) {
        return crate::theme::SYN_VARIABLE(); // VSCode's JSON-key blue
    }
    match scopes.last() {
        Some(scope) => scope_color(&scope_string(*scope)),
        None => crate::theme::FG_TEXT(),
    }
}

/// Build the dotted string for a syntect `Scope` (via the global scope repo).
fn scope_string(scope: Scope) -> String {
    scope.build_string()
}

/// JSON key coloring (#37): the bundled grammar scopes object keys and string
/// values identically, so a whole JSON document reads as one uniform color.
/// JSON's structure is unambiguous — a string followed by `:` IS a key — so
/// recolor those ranges (quotes included, like VSCode's property-name blue).
fn recolor_json_keys(line: &str, spans: &mut Vec<(String, Color)>) {
    let bytes = line.as_bytes();
    let mut keys: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'"' {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        while i < bytes.len() && bytes[i] != b'"' {
            i += if bytes[i] == b'\\' { 2 } else { 1 };
        }
        if i >= bytes.len() {
            break; // unterminated string on this line
        }
        let end = i + 1; // past the closing quote
        let mut j = end;
        while j < bytes.len() && matches!(bytes[j], b' ' | b'\t') {
            j += 1;
        }
        if j < bytes.len() && bytes[j] == b':' {
            keys.push((start, end));
        }
        i = end;
    }
    if keys.is_empty() {
        return;
    }
    let key_color = crate::theme::SYN_VARIABLE();
    let mut out: Vec<(String, Color)> = Vec::with_capacity(spans.len() + keys.len() * 2);
    let mut off = 0usize;
    for (s, c) in spans.drain(..) {
        let s_end = off + s.len();
        let mut cur = off;
        for &(klo, khi) in &keys {
            if khi <= cur || klo >= s_end {
                continue;
            }
            let lo = klo.max(cur);
            let hi = khi.min(s_end);
            if lo > cur {
                out.push((s[(cur - off)..(lo - off)].to_string(), c));
            }
            out.push((s[(lo - off)..(hi - off)].to_string(), key_color));
            cur = hi;
        }
        if cur < s_end {
            out.push((s[(cur - off)..].to_string(), c));
        }
        off = s_end;
    }
    *spans = out;
}

/// Per-document incremental tokenizer state: the parse state + scope stack at the
/// START of each line, so editing line N only re-tokenizes from N forward until
/// the carried state matches the cached state (reconvergence).
pub struct LineCache {
    ext: String,
    /// (ParseState, ScopeStack) snapshot at the start of line `i`.
    starts: Vec<(ParseState, ScopeStack)>,
    /// Cached colored spans (substring, color) for line `i`, including its `\n`.
    spans: Vec<Vec<(String, Color)>>,
    /// Exclusive end of the line range the last `highlight` actually re-tokenized
    /// (start = the dirty line it was given). Drives the editor's partial buffer
    /// reshape: lines outside this range kept their cached spans verbatim.
    last_end: usize,
}

impl LineCache {
    pub fn new(ext: &str) -> Option<LineCache> {
        let syntax = syntax_for(ext)?;
        Some(LineCache {
            ext: ext.to_string(),
            starts: vec![(ParseState::new(syntax), ScopeStack::new())],
            spans: Vec::new(),
            last_end: 0,
        })
    }

    /// Exclusive end of the last pass's re-tokenized line range.
    pub fn last_changed_end(&self) -> usize {
        self.last_end
    }

    /// The cached spans for one line (substring, color), if tokenized.
    pub fn line_spans(&self, line: usize) -> Option<&[(String, Color)]> {
        self.spans.get(line).map(|v| v.as_slice())
    }

    /// Number of cached lines (display-text lines, as cosmic-text counts them —
    /// a trailing '\n' does NOT produce an extra empty line).
    pub fn line_count(&self) -> usize {
        self.spans.len()
    }

    /// Re-tokenize `text` from `dirty_line` onward, reusing cached line states
    /// before it. Returns the full document's rich-text spans (concatenated lines).
    /// Pass `dirty_line = 0` for a fresh document.
    ///
    /// CONVERGENCE: re-parsing stops as soon as a line's resulting parse state
    /// matches the cached state of the next line (and the line count is
    /// unchanged) — every later line would tokenize identically, so its cached
    /// spans are reused. Without this, one keystroke re-parses from the edit to
    /// the END of the file: in a ~6000-line .rs file that's seconds of syntect
    /// per letter — the app "freezes" while typing (#36).
    pub fn highlight(&mut self, text: &str, dirty_line: usize) -> Vec<(String, Color)> {
        self.refresh(text, dirty_line);
        self.flatten()
    }

    /// The cached whole-document spans, flattened (the `highlight` return shape).
    pub fn flatten(&self) -> Vec<(String, Color)> {
        self.spans.iter().flatten().cloned().collect()
    }

    /// Re-tokenize without flattening; returns the `[start, end)` line range that
    /// was actually re-tokenized (callers can rebuild just those buffer lines).
    pub fn refresh(&mut self, text: &str, dirty_line: usize) -> (usize, usize) {
        let ss = syntax_set();
        let lines: Vec<&str> = LinesWithEndingsIter::new(text).collect();
        let start = dirty_line.min(self.starts.len().saturating_sub(1));
        // Convergence needs the cached tail to correspond 1:1 to the current
        // text. When the line count CHANGED (Enter / paste / delete-line), align
        // the caches first: insert placeholders at the edit for added lines, or
        // drop the removed ones — the shifted tail then lines up again, so the
        // re-parse still stops at convergence instead of running to EOF.
        let old_count = self.spans.len();
        let new_count = lines.len();
        let mut no_break_before = start;
        if old_count != 0 && new_count != old_count && start < new_count {
            if new_count > old_count {
                let k = new_count - old_count;
                let at = start.min(self.spans.len());
                for _ in 0..k {
                    self.spans.insert(at, Vec::new());
                }
                let st_idx = (at + 1).min(self.starts.len() - 1);
                let st = self.starts[st_idx].clone();
                for _ in 0..k {
                    self.starts.insert(at + 1, st.clone());
                }
                // The inserted lines (and the split line itself) must all be
                // re-parsed before convergence may stop the loop.
                no_break_before = start + k;
            } else {
                let k = old_count - new_count;
                for _ in 0..k {
                    if start < self.spans.len() {
                        self.spans.remove(start);
                    }
                    if start + 1 < self.starts.len() {
                        self.starts.remove(start + 1);
                    }
                }
            }
        }
        let counts_match = self.spans.len() == lines.len();

        for i in start..lines.len() {
            let (mut state, mut stack) = self.starts[i].clone();
            let line = lines[i];
            let mut line_spans: Vec<(String, Color)> = Vec::new();
            if let Ok(ops) = state.parse_line(line, ss) {
                let mut last = 0usize;
                for (idx, op) in ops {
                    if idx > last {
                        line_spans.push((line[last..idx].to_string(), color_for_stack(&stack)));
                    }
                    stack.apply(&op).ok();
                    last = idx;
                }
                if last < line.len() {
                    line_spans.push((line[last..].to_string(), color_for_stack(&stack)));
                }
            } else {
                line_spans.push((line.to_string(), crate::theme::FG_TEXT()));
            }
            if matches!(self.ext.as_str(), "json" | "jsonc" | "mcp") {
                recolor_json_keys(line, &mut line_spans);
            }
            if i < self.spans.len() {
                self.spans[i] = line_spans;
            } else {
                self.spans.push(line_spans);
            }
            // Snapshot the state at the start of the NEXT line; stop once it
            // matches the cached snapshot (the tail can't tokenize differently).
            let next = (state, stack);
            self.last_end = i + 1;
            if i + 1 < self.starts.len() {
                if counts_match && i >= start && i + 1 > no_break_before && self.starts[i + 1] == next {
                    break;
                }
                self.starts[i + 1] = next;
            } else {
                self.starts.push(next);
            }
        }
        // Drop any trailing caches if the document shrank.
        self.spans.truncate(lines.len());
        self.starts.truncate(lines.len() + 1);
        self.last_end = self.last_end.min(lines.len());
        (start, self.last_end.max(start))
    }

    pub fn ext(&self) -> &str {
        &self.ext
    }
}

/// Same shape as one line's cached spans for two `(text, color)` rows.
fn line_eq(a: &[(String, Color)], b: &[(String, Color)]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|((sa, ca), (sb, cb))| {
            sa == sb && ca.r() == cb.r() && ca.g() == cb.g() && ca.b() == cb.b() && ca.a() == cb.a()
        })
}

/// Tree-sitter–backed line cache for JS/TS — a drop-in alternative to the syntect
/// [`LineCache`] for languages whose TextMate grammar misbehaves (template
/// literals, regex). Tree-sitter parses the whole document each refresh (cheap for
/// the non-huge files we highlight) and we diff against the cache to report the
/// minimal changed line range, so the editor still reshapes only what moved.
pub struct TsCache {
    lang: crate::syntax::Lang,
    spans: Vec<Vec<(String, Color)>>,
    last_end: usize,
}

impl TsCache {
    pub fn new(ext: &str) -> Option<TsCache> {
        let lang = crate::syntax::Lang::from_ext(ext);
        matches!(lang, crate::syntax::Lang::TypeScript | crate::syntax::Lang::Tsx)
            .then(|| TsCache { lang, spans: Vec::new(), last_end: 0 })
    }

    pub fn refresh(&mut self, text: &str, _dirty: usize) -> (usize, usize) {
        let new = crate::syntax::highlight_lines(self.lang, text).unwrap_or_default();
        // Minimal changed range: skip the matching prefix + suffix vs the old cache.
        let mut start = 0;
        while start < self.spans.len() && start < new.len() && line_eq(&self.spans[start], &new[start]) {
            start += 1;
        }
        let (mut eo, mut en) = (self.spans.len(), new.len());
        while eo > start && en > start && line_eq(&self.spans[eo - 1], &new[en - 1]) {
            eo -= 1;
            en -= 1;
        }
        self.spans = new;
        let start = start.min(self.spans.len());
        self.last_end = en.max(start).min(self.spans.len());
        (start, self.last_end)
    }

    pub fn highlight(&mut self, text: &str, dirty: usize) -> Vec<(String, Color)> {
        self.refresh(text, dirty);
        self.flatten()
    }

    pub fn flatten(&self) -> Vec<(String, Color)> {
        self.spans.iter().flatten().cloned().collect()
    }

    pub fn line_count(&self) -> usize {
        self.spans.len()
    }

    pub fn line_spans(&self, line: usize) -> Option<&[(String, Color)]> {
        self.spans.get(line).map(|v| v.as_slice())
    }

    pub fn last_changed_end(&self) -> usize {
        self.last_end
    }
}

/// The active highlighter for a document: tree-sitter for JS/TS (robust template
/// literal / regex handling), syntect for everything else. Both expose the same
/// per-line span API the editor consumes.
pub enum Highlighter {
    Syntect(LineCache),
    Tree(TsCache),
}

impl Highlighter {
    /// Tree-sitter for JS/TS-family extensions; otherwise the syntect grammar (if
    /// one is bundled for `ext`). `None` ⇒ no highlighter (plain text).
    pub fn new(ext: &str) -> Option<Highlighter> {
        if let Some(ts) = TsCache::new(ext) {
            return Some(Highlighter::Tree(ts));
        }
        LineCache::new(ext).map(Highlighter::Syntect)
    }

    pub fn refresh(&mut self, text: &str, dirty: usize) -> (usize, usize) {
        match self {
            Highlighter::Syntect(c) => c.refresh(text, dirty),
            Highlighter::Tree(c) => c.refresh(text, dirty),
        }
    }

    pub fn highlight(&mut self, text: &str, dirty: usize) -> Vec<(String, Color)> {
        match self {
            Highlighter::Syntect(c) => c.highlight(text, dirty),
            Highlighter::Tree(c) => c.highlight(text, dirty),
        }
    }

    pub fn flatten(&self) -> Vec<(String, Color)> {
        match self {
            Highlighter::Syntect(c) => c.flatten(),
            Highlighter::Tree(c) => c.flatten(),
        }
    }

    pub fn line_count(&self) -> usize {
        match self {
            Highlighter::Syntect(c) => c.line_count(),
            Highlighter::Tree(c) => c.line_count(),
        }
    }

    pub fn line_spans(&self, line: usize) -> Option<&[(String, Color)]> {
        match self {
            Highlighter::Syntect(c) => c.line_spans(line),
            Highlighter::Tree(c) => c.line_spans(line),
        }
    }

    pub fn last_changed_end(&self) -> usize {
        match self {
            Highlighter::Syntect(c) => c.last_changed_end(),
            Highlighter::Tree(c) => c.last_changed_end(),
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn rust_doc(lines: usize) -> String {
        let mut out = String::new();
        for i in 0..lines {
            out.push_str(&format!("fn func_{i}() {{ let x = {i}; /* body */ println!(\"{{}}\", x); }}\n"));
        }
        out
    }

    // #37: JSON keys and values must be DIFFERENT colors (VSCode: property-name
    // blue vs string orange) — not one uniform string color across the document.
    #[test]
    fn json_keys_and_values_color_differently() {
        let src = "{\n  \"name\": \"aether\",\n  \"count\": 42,\n  \"on\": true\n}\n";
        let mut cache = LineCache::new("json").expect("json grammar");
        let spans = cache.highlight(src, 0);
        let color_of = |needle: &str| {
            spans
                .iter()
                .find(|(s, _)| s.contains(needle))
                .map(|(_, c)| *c)
                .unwrap_or_else(|| panic!("token {needle:?} not found in {spans:?}"))
        };
        let key = color_of("name");
        let val = color_of("aether");
        assert_ne!(key, val, "JSON key vs string value must differ");
        assert_eq!(key, crate::theme::SYN_VARIABLE(), "keys use property-name color");
        assert_eq!(val, crate::theme::SYN_STRING(), "values stay string-colored");
        // Numbers / booleans keep their own colors too.
        assert_eq!(color_of("42"), crate::theme::SYN_NUMBER());
        assert_ne!(color_of("true"), val);
    }

    // Editing one line must NOT re-tokenize the rest of the file (#36: one
    // keystroke in a ~6k-line .rs froze the app). Correctness: the converged
    // incremental result must equal a from-scratch highlight. Performance: the
    // incremental pass must be drastically faster than the initial full pass.
    #[test]
    fn highlight_converges_after_single_line_edit() {
        let n = 6000;
        let mut text = rust_doc(n);
        let mut cache = LineCache::new("rs").expect("rust grammar");
        let t0 = std::time::Instant::now();
        cache.highlight(&text, 0);
        let full = t0.elapsed();

        // Same-length edit on line 100 (replace 'x' with 'y' once).
        let line_start: usize = text.lines().take(100).map(|l| l.len() + 1).sum();
        let off = line_start + text[line_start..].find('x').unwrap();
        text.replace_range(off..off + 1, "y");

        let t1 = std::time::Instant::now();
        let inc = cache.highlight(&text, 100);
        let inc_time = t1.elapsed();

        // Correctness: identical to a fresh highlight of the edited text.
        let mut fresh = LineCache::new("rs").unwrap();
        let want = fresh.highlight(&text, 0);
        assert_eq!(inc, want, "incremental spans must match a fresh highlight");

        // Perf: convergence should make the edit pass at least 20x faster than
        // the initial full tokenize (in practice it is hundreds of times).
        assert!(
            inc_time < full / 20,
            "incremental highlight too slow: {inc_time:?} vs full {full:?}"
        );
    }
}










