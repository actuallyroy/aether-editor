// Local relevance-ranked search — the `searchWorkspace`/`searchTerminal` MCP tools.
// Deliberately NOT embeddings/vector search: BM25-flavored keyword + phrase ranking
// (tokenize, drop stopwords, phrase-match on "quoted text", score by term frequency)
// is instant, free, fully local, and gives exact recall on identifiers — which is
// what "find the thing" queries actually need. Modeled on the same approach the
// `project-manager` MCP server uses for its `search_transcripts` tool.

use std::path::Path;

const STOPWORDS: &[&str] = &[
    "a", "an", "and", "the", "of", "to", "in", "on", "for", "is", "it", "its", "as", "at", "be",
    "by", "or", "if", "no", "not", "this", "that", "these", "those", "with", "from", "into",
    "about", "over", "under", "then", "than", "so", "we", "i", "you", "he", "she", "they", "them",
    "us", "our", "your", "my", "me", "do", "did", "does", "done", "can", "could", "should",
    "would", "will", "what", "which", "who", "when", "where", "why", "how", "here", "there",
    "all", "any", "one", "more", "most", "some", "such", "only", "just", "also", "new", "use",
    "used", "using", "get", "got", "make", "made", "see", "want", "need", "yes", "like",
];

const MAX_FILE_BYTES: u64 = 2_000_000;
const MAX_CANDIDATES: usize = 3000; // stop scanning past this many scored hits
const MAX_FILES_SCANNED: usize = 20_000;

pub struct Ranker {
    terms: Vec<String>,
    phrases: Vec<String>,
}

impl Ranker {
    pub fn new(query: &str) -> Self {
        let (phrases, rest) = extract_phrases(query);
        let terms: Vec<String> = tokenize(&rest).into_iter().filter(|t| !STOPWORDS.contains(&t.as_str())).collect();
        Self { terms, phrases }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty() && self.phrases.is_empty()
    }

    /// Score one line (already lowercased by the caller isn't required — this
    /// lowercases internally). Returns `None` if nothing matched.
    fn score(&self, line: &str) -> Option<f32> {
        let lower = line.to_lowercase();
        let mut score = 0.0;
        for p in &self.phrases {
            if lower.contains(p.as_str()) {
                score += 5.0;
            }
        }
        for t in &self.terms {
            let count = lower.matches(t.as_str()).count();
            if count > 0 {
                score += 1.0 + (count as f32 - 1.0) * 0.3;
            }
        }
        (score > 0.0).then_some(score)
    }
}

/// Pull out `"quoted phrases"` (lowercased, required-exact) from a query, returning
/// them plus the remainder of the query (for term tokenization).
fn extract_phrases(query: &str) -> (Vec<String>, String) {
    let mut phrases = Vec::new();
    let mut rest = String::new();
    let mut chars = query.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '"' {
            let mut phrase = String::new();
            for c2 in chars.by_ref() {
                if c2 == '"' {
                    break;
                }
                phrase.push(c2);
            }
            if !phrase.trim().is_empty() {
                phrases.push(phrase.to_lowercase());
            }
        } else {
            rest.push(c);
        }
    }
    (phrases, rest)
}

fn tokenize(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            cur.push(c.to_ascii_lowercase());
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

pub struct Hit {
    pub file: String,
    pub line: usize,
    pub score: f32,
    pub text: String,
}

/// Search every line of every file under `root` (skipping the usual VCS/build/dep
/// junk — same list the file tree and find-in-files use), ranked by `ranker`.
/// Returns hits sorted best-first, plus whether scanning stopped early (candidate
/// or file-count cap hit) rather than having covered everything.
pub fn search_workspace(root: &Path, workspace_root: &Path, ranker: &Ranker) -> (Vec<Hit>, bool) {
    let mut hits: Vec<Hit> = Vec::new();
    let mut files_scanned = 0usize;
    let mut truncated = false;
    let mut stack = vec![root.to_path_buf()];
    'walk: while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for entry in rd.flatten() {
            let path = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !crate::search::SKIP_DIRS.contains(&name.as_str()) {
                    stack.push(path);
                }
                continue;
            }
            if !ft.is_file() {
                continue;
            }
            files_scanned += 1;
            if files_scanned > MAX_FILES_SCANNED {
                truncated = true;
                break 'walk;
            }
            if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > MAX_FILE_BYTES {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else { continue };
            if bytes.iter().take(8000).any(|&b| b == 0) {
                continue; // looks binary
            }
            let Ok(text) = String::from_utf8(bytes) else { continue };
            let rel = path.strip_prefix(workspace_root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
            for (i, line) in text.lines().enumerate() {
                if let Some(score) = ranker.score(line) {
                    hits.push(Hit { file: rel.clone(), line: i + 1, score, text: line.trim().to_string() });
                    if hits.len() >= MAX_CANDIDATES {
                        truncated = true;
                        break 'walk;
                    }
                }
            }
        }
    }
    hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    (hits, truncated)
}

/// Rank an already-materialized line stream (e.g. one terminal's scrollback).
pub fn search_lines<'a>(lines: impl Iterator<Item = &'a str>, ranker: &Ranker) -> Vec<Hit> {
    let mut hits: Vec<Hit> = lines
        .enumerate()
        .filter_map(|(i, line)| ranker.score(line).map(|score| Hit { file: String::new(), line: i + 1, score, text: line.to_string() }))
        .collect();
    hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    hits
}
