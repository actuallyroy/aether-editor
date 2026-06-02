// Find-in-files: walk the workspace on a worker thread and stream matches back to
// the UI. All option combinations (case / whole-word / regex) are expressed as a
// single `fancy_regex` pattern so the matching path is uniform.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::Sender;

use fancy_regex::Regex;

use crate::marketplace::WorkerMsg;

#[derive(Clone, Copy, Default, PartialEq)]
pub struct SearchOpts {
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
}

/// One matching line within a file: the (1-based) line number, its text, and the
/// byte ranges within that text that matched (for highlighting).
#[derive(Clone)]
pub struct LineMatch {
    pub line: usize,
    pub text: String,
    pub ranges: Vec<(usize, usize)>,
}

/// All matches within a single file.
#[derive(Clone)]
pub struct FileMatches {
    pub path: PathBuf,
    pub rel: String, // path relative to the workspace root, for display
    pub lines: Vec<LineMatch>,
}

/// A flattened display row: a collapsible file header (`line == None`) or a match
/// line (`line == Some(1-based)`). `ranges` are byte ranges in `text` to highlight.
pub struct SearchRow {
    pub file: usize,
    pub line: Option<usize>,
    pub text: String,
    pub ranges: Vec<(usize, usize)>, // byte ranges in `text` to highlight
    pub col: usize,                  // byte offset of the first match in the original line
}

/// Flatten results into display rows: a header per file, then (unless that file is
/// collapsed) its match lines with leading whitespace trimmed and match ranges
/// re-based onto the displayed text. No line numbers — VSCode-style.
pub fn build_rows(results: &[FileMatches], collapsed: &HashSet<usize>) -> Vec<SearchRow> {
    const INDENT: &str = "    ";
    let mut rows = Vec::new();
    for (fi, f) in results.iter().enumerate() {
        // Leading spaces leave room for the codicon chevron drawn over them.
        rows.push(SearchRow {
            file: fi,
            line: None,
            text: format!("    {}  ({})", f.rel, f.lines.len()),
            ranges: Vec::new(),
            col: 0,
        });
        if collapsed.contains(&fi) {
            continue;
        }
        for lm in &f.lines {
            let lead = lm.text.len() - lm.text.trim_start().len();
            let text = format!("{INDENT}{}", &lm.text[lead..]);
            let max = text.len();
            let off = INDENT.len() as isize - lead as isize;
            let ranges = lm
                .ranges
                .iter()
                .filter_map(|&(s, e)| {
                    let s2 = (s as isize + off).max(INDENT.len() as isize) as usize;
                    let e2 = ((e as isize + off).max(0) as usize).min(max);
                    (e2 > s2).then_some((s2, e2))
                })
                .collect();
            let col = lm.ranges.first().map(|&(s, _)| s).unwrap_or(0);
            rows.push(SearchRow { file: fi, line: Some(lm.line), text, ranges, col });
        }
    }
    rows
}

/// Directories never worth searching, and a per-file size ceiling.
const SKIP_DIRS: &[&str] = &[".git", "target", "node_modules", ".aether", "dist", "build", "out"];
const MAX_FILE_BYTES: u64 = 2_000_000;
const MAX_TOTAL_MATCHES: usize = 5000;
const BATCH_FILES: usize = 12;

/// Escape regex metacharacters so a plain query matches literally.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        if "\\^$.|?*+()[]{}".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Compile the query + options into one regex (None when the query is empty or the
/// user-supplied regex is invalid).
pub fn build_regex(query: &str, opts: SearchOpts) -> Option<Regex> {
    if query.is_empty() {
        return None;
    }
    let mut pat = if opts.regex { query.to_string() } else { escape(query) };
    if opts.whole_word {
        pat = format!(r"\b(?:{pat})\b");
    }
    if !opts.case_sensitive {
        pat = format!("(?i){pat}");
    }
    Regex::new(&pat).ok()
}

fn line_ranges(re: &Regex, line: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut start = 0;
    while start <= line.len() {
        match re.find_from_pos(line, start) {
            Ok(Some(m)) => {
                ranges.push((m.start(), m.end()));
                start = if m.end() > m.start() { m.end() } else { m.end() + 1 };
            }
            _ => break,
        }
    }
    ranges
}

/// Search `root` recursively for `query` on a background thread, streaming batches
/// of `FileMatches` over `tx` (tagged with `gen` so stale results can be ignored),
/// then a final `SearchDone`.
pub fn search_async(tx: Sender<WorkerMsg>, gen: u64, root: PathBuf, query: String, opts: SearchOpts) {
    std::thread::spawn(move || {
        let Some(re) = build_regex(&query, opts) else {
            let _ = tx.send(WorkerMsg::SearchDone { gen });
            return;
        };
        let mut batch: Vec<FileMatches> = Vec::new();
        let mut total = 0usize;
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for entry in rd.flatten() {
                let path = entry.path();
                let Ok(ft) = entry.file_type() else { continue };
                if ft.is_dir() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if !SKIP_DIRS.contains(&name.as_str()) {
                        stack.push(path);
                    }
                    continue;
                }
                if !ft.is_file() {
                    continue;
                }
                if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > MAX_FILE_BYTES {
                    continue;
                }
                let Ok(bytes) = std::fs::read(&path) else { continue };
                if bytes.iter().take(8000).any(|&b| b == 0) {
                    continue; // looks binary
                }
                let Ok(text) = String::from_utf8(bytes) else { continue };
                let mut lines = Vec::new();
                for (i, line) in text.lines().enumerate() {
                    let ranges = line_ranges(&re, line);
                    if !ranges.is_empty() {
                        lines.push(LineMatch { line: i + 1, text: line.to_string(), ranges });
                        total += 1;
                        if lines.len() >= 200 {
                            break;
                        }
                    }
                }
                if !lines.is_empty() {
                    let rel = path
                        .strip_prefix(&root)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .replace('\\', "/");
                    batch.push(FileMatches { path: path.clone(), rel, lines });
                    if batch.len() >= BATCH_FILES {
                        let _ = tx.send(WorkerMsg::SearchHits { gen, files: std::mem::take(&mut batch) });
                    }
                }
                if total >= MAX_TOTAL_MATCHES {
                    if !batch.is_empty() {
                        let _ = tx.send(WorkerMsg::SearchHits { gen, files: std::mem::take(&mut batch) });
                    }
                    let _ = tx.send(WorkerMsg::SearchDone { gen });
                    return;
                }
            }
        }
        if !batch.is_empty() {
            let _ = tx.send(WorkerMsg::SearchHits { gen, files: batch });
        }
        let _ = tx.send(WorkerMsg::SearchDone { gen });
    });
}

/// Replace every match of `query` (with `opts`) by `replacement` across the given
/// files, rewriting them on disk. Returns the number of files changed. Runs inline
/// (called from the UI thread on an explicit "Replace All"); the set of files is
/// already known from the last search so this is bounded.
pub fn replace_all(files: &[FileMatches], query: &str, opts: SearchOpts, replacement: &str) -> usize {
    let Some(re) = build_regex(query, opts) else { return 0 };
    let mut changed = 0;
    for f in files {
        let Ok(orig) = std::fs::read_to_string(&f.path) else { continue };
        let new = re.replace_all(&orig, replacement);
        if new != orig {
            if std::fs::write(&f.path, new.as_ref()).is_ok() {
                changed += 1;
            }
        }
    }
    changed
}
