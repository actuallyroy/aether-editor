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

/// Include / exclude path globs, comma-separated (VSCode-style: `*.rs, src/**`).
/// Compiled once per search. An empty include set matches everything; any matching
/// exclude glob drops the file.
#[derive(Default)]
pub struct Filters {
    include: Vec<Regex>,
    exclude: Vec<Regex>,
}

impl Filters {
    pub fn new(include: &str, exclude: &str) -> Self {
        Self { include: compile_globs(include), exclude: compile_globs(exclude) }
    }
    /// Exclude-only filter from a list of glob patterns (e.g. VSCode `files.exclude`).
    pub fn exclude_globs(patterns: &[String]) -> Self {
        let exclude = patterns
            .iter()
            .filter_map(|p| Regex::new(&glob_to_regex(p.trim())).ok())
            .collect();
        Self { include: Vec::new(), exclude }
    }
    /// Does this repo-relative (forward-slash) path pass the filters?
    pub fn allows(&self, rel: &str) -> bool {
        if !self.include.is_empty() && !self.include.iter().any(|re| re.is_match(rel).unwrap_or(false)) {
            return false;
        }
        !self.exclude.iter().any(|re| re.is_match(rel).unwrap_or(false))
    }
}

/// Compile a comma-separated glob list into anchored regexes. Patterns with no `/`
/// match against any path segment (so `*.rs` finds files anywhere); patterns with a
/// `/` anchor from the path root.
fn compile_globs(patterns: &str) -> Vec<Regex> {
    patterns
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .filter_map(|p| Regex::new(&glob_to_regex(p)).ok())
        .collect()
}

fn glob_to_regex(glob: &str) -> String {
    let mut re = String::new();
    let has_slash = glob.contains('/');
    // No slash → match the basename / any segment; slash → anchor at the root.
    re.push_str(if has_slash { "^" } else { "(^|/)" });
    let bytes: Vec<char> = glob.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            '*' => {
                if i + 1 < bytes.len() && bytes[i + 1] == '*' {
                    re.push_str(".*"); // ** spans directories
                    i += 1;
                } else {
                    re.push_str("[^/]*"); // * stays within a segment
                }
            }
            '?' => re.push_str("[^/]"),
            '.' | '+' | '(' | ')' | '|' | '^' | '$' | '{' | '}' | '[' | ']' | '\\' => {
                re.push('\\');
                re.push(c);
            }
            _ => re.push(c),
        }
        i += 1;
    }
    re.push('$');
    re
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

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Dir,
    File,
    Match,
}

/// One row of the results *folder tree* (VSCode "view as tree"): directory groups,
/// file nodes, and match lines, each at a `depth`. Directories and files are
/// collapsible via their `key`; match rows carry the highlight `ranges`.
pub struct TreeRow {
    pub depth: usize,
    pub kind: RowKind,
    pub label: String,            // dir/file name (+ count) or the trimmed match text
    pub file: usize,              // result index (File / Match rows)
    pub rel: String,              // file's repo-relative path (File rows — for the icon)
    pub line: Option<usize>,      // 1-based line (Match rows)
    pub col: usize,               // first-match byte offset in the original line
    pub ranges: Vec<(usize, usize)>, // highlight ranges in `label` (Match rows)
    pub key: String,              // collapse id ("d:<path>" / "f:<rel>"); empty for matches
}

#[derive(Default)]
struct Dir {
    subdirs: std::collections::BTreeMap<String, Dir>,
    files: Vec<usize>, // indices into `results`, sorted by basename at emit time
}

fn count_matches(node: &Dir, results: &[FileMatches]) -> usize {
    let mut n: usize = node.files.iter().map(|&fi| results[fi].lines.len()).sum();
    for sub in node.subdirs.values() {
        n += count_matches(sub, results);
    }
    n
}

/// Build the results as a collapsible directory tree. Single-child directory chains
/// are compressed (`renderer/src` shown as one node), matching VSCode.
pub fn build_tree_rows(results: &[FileMatches], collapsed: &HashSet<String>) -> Vec<TreeRow> {
    // 1. Bucket files into a directory trie.
    let mut root = Dir::default();
    for (fi, f) in results.iter().enumerate() {
        let mut segs: Vec<&str> = f.rel.split('/').collect();
        segs.pop(); // drop the filename — the leaf is the file itself
        let mut node = &mut root;
        for seg in segs {
            node = node.subdirs.entry(seg.to_string()).or_default();
        }
        node.files.push(fi);
    }
    // 2. Depth-first emit.
    let mut rows = Vec::new();
    emit_children(&root, "", 0, collapsed, results, &mut rows);
    rows
}

fn emit_children(node: &Dir, base: &str, depth: usize, collapsed: &HashSet<String>, results: &[FileMatches], rows: &mut Vec<TreeRow>) {
    for (seg, child) in &node.subdirs {
        emit_dir(child, seg.clone(), join_path(base, seg), depth, collapsed, results, rows);
    }
    let mut files = node.files.clone();
    files.sort_by(|&a, &b| basename(&results[a].rel).cmp(basename(&results[b].rel)));
    for fi in files {
        emit_file(fi, depth, collapsed, results, rows);
    }
}

fn emit_dir(node: &Dir, mut name: String, mut path: String, depth: usize, collapsed: &HashSet<String>, results: &[FileMatches], rows: &mut Vec<TreeRow>) {
    // Compress single-child dir chains: `renderer` + `src` -> `renderer/src`.
    let mut node = node;
    while node.subdirs.len() == 1 && node.files.is_empty() {
        let (seg, child) = node.subdirs.iter().next().unwrap();
        name = format!("{name}/{seg}");
        path = join_path(&path, seg);
        node = child;
    }
    let key = format!("d:{path}");
    rows.push(TreeRow {
        depth,
        kind: RowKind::Dir,
        label: format!("{name}  ({})", count_matches(node, results)),
        file: 0,
        rel: String::new(),
        line: None,
        col: 0,
        ranges: Vec::new(),
        key: key.clone(),
    });
    if collapsed.contains(&key) {
        return;
    }
    emit_children(node, &path, depth + 1, collapsed, results, rows);
}

fn emit_file(fi: usize, depth: usize, collapsed: &HashSet<String>, results: &[FileMatches], rows: &mut Vec<TreeRow>) {
    let f = &results[fi];
    let key = format!("f:{}", f.rel);
    rows.push(TreeRow {
        depth,
        kind: RowKind::File,
        label: format!("{}  ({})", basename(&f.rel), f.lines.len()),
        file: fi,
        rel: f.rel.clone(),
        line: None,
        col: 0,
        ranges: Vec::new(),
        key: key.clone(),
    });
    if collapsed.contains(&key) {
        return;
    }
    for lm in &f.lines {
        let lead = lm.text.len() - lm.text.trim_start().len();
        let text = lm.text[lead..].to_string();
        let max = text.len();
        let ranges = lm
            .ranges
            .iter()
            .filter_map(|&(s, e)| {
                let s2 = s.saturating_sub(lead);
                let e2 = e.saturating_sub(lead).min(max);
                (e2 > s2).then_some((s2, e2))
            })
            .collect();
        let col = lm.ranges.first().map(|&(s, _)| s).unwrap_or(0);
        rows.push(TreeRow {
            depth: depth + 1,
            kind: RowKind::Match,
            label: text,
            file: fi,
            rel: String::new(),
            line: Some(lm.line),
            col,
            ranges,
            key: String::new(),
        });
    }
}

/// Flat (non-tree) view: every file at depth 0 with its matches at depth 1, no
/// directory grouping — VSCode's "view as list".
pub fn build_flat_rows(results: &[FileMatches], collapsed: &HashSet<String>) -> Vec<TreeRow> {
    let mut rows = Vec::new();
    for fi in 0..results.len() {
        emit_file(fi, 0, collapsed, results, &mut rows);
    }
    rows
}

/// Every collapse key in the tree (for "collapse all").
pub fn all_group_keys(results: &[FileMatches]) -> HashSet<String> {
    let mut keys = HashSet::new();
    for f in results {
        keys.insert(format!("f:{}", f.rel));
        let mut segs: Vec<&str> = f.rel.split('/').collect();
        segs.pop();
        let mut path = String::new();
        for seg in segs {
            path = join_path(&path, seg);
            keys.insert(format!("d:{path}"));
        }
    }
    keys
}

fn basename(rel: &str) -> &str {
    rel.rsplit('/').next().unwrap_or(rel)
}

fn join_path(base: &str, seg: &str) -> String {
    if base.is_empty() {
        seg.to_string()
    } else {
        format!("{base}/{seg}")
    }
}

/// Directories never worth indexing or searching — VCS metadata plus the usual
/// generated/build/dependency output. Shared by project search and the command
/// palette's file picker so both exclude the same garbage (single source of truth).
pub const SKIP_DIRS: &[&str] = &[
    ".git", ".svn", ".hg", "target", "node_modules", ".aether", "dist", "build", "out",
    ".next", ".nuxt", ".svelte-kit", ".venv", "venv", "bin", "obj", "Pods", ".expo",
    "__pycache__", ".gradle", "DerivedData", "coverage", ".cache",
];
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
pub fn search_async(tx: Sender<WorkerMsg>, gen: u64, root: PathBuf, query: String, opts: SearchOpts, filters: Filters) {
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
                let rel = path
                    .strip_prefix(&root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                if !filters.allows(&rel) {
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
