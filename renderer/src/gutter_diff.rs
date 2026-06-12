// Per-line change indicators for the editor gutter (VSCode-style): diff the open
// buffer against its HEAD baseline and mark each current line as Added / Modified,
// or place a Deleted marker where lines were removed. Computed in-memory so it
// tracks unsaved edits live; the baseline blob is fetched once off-thread.
//
// `hunks()` is the source of truth — it returns each change region with the original
// (HEAD) lines it replaced, which drives both the gutter marks and the inline peek
// (clicking a bar shows the original lines, VSCode-style).

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Change {
    Added,    // a new line with no counterpart in the baseline (green)
    Modified, // a line that replaced a baseline line (blue)
    Deleted,  // baseline lines removed here — marker sits on the current line (red)
}

/// One change region. `cur_lo..cur_hi` is the current-line range that changed
/// (empty range ⇒ a pure deletion anchored at `cur_lo`); `base` holds the original
/// HEAD lines this hunk replaced or removed (empty ⇒ a pure addition).
#[derive(Clone, Debug)]
pub struct Hunk {
    pub cur_lo: usize,
    pub cur_hi: usize,
    pub base: Vec<String>,
}

impl Hunk {
    pub fn kind(&self) -> Change {
        if self.cur_hi > self.cur_lo {
            if self.base.is_empty() { Change::Added } else { Change::Modified }
        } else {
            Change::Deleted
        }
    }
    /// True when this hunk owns current line `line` (the deletion marker line for
    /// pure deletions, otherwise any line in its current range).
    pub fn contains(&self, line: usize) -> bool {
        if self.cur_hi > self.cur_lo {
            (self.cur_lo..self.cur_hi).contains(&line)
        } else {
            line == self.cur_lo
        }
    }
}

/// Per-line marks for the gutter, derived from `hunks`.
pub fn compute(base: &[String], cur: &[String]) -> Vec<(usize, Change)> {
    let mut out = Vec::new();
    for h in hunks(base, cur) {
        match h.kind() {
            Change::Deleted => out.push((h.cur_lo, Change::Deleted)),
            kind => out.extend((h.cur_lo..h.cur_hi).map(|l| (l, kind))),
        }
    }
    out
}

/// The hunk owning current line `line`, if any.
pub fn hunk_at(base: &[String], cur: &[String], line: usize) -> Option<Hunk> {
    hunks(base, cur).into_iter().find(|h| h.contains(line))
}

/// Diff `base` (HEAD) against `cur` (live buffer) into change regions. Trims the
/// common prefix/suffix first (so ordinary local edits diff a tiny middle), then
/// runs an LCS edit-script on the differing middle. Whole-file rewrites past a size
/// cap fall back to one coarse modified hunk rather than a huge DP table.
pub fn hunks(base: &[String], cur: &[String]) -> Vec<Hunk> {
    let (n, m) = (base.len(), cur.len());
    let mut out = Vec::new();
    let mut p = 0;
    while p < n && p < m && base[p] == cur[p] {
        p += 1;
    }
    let mut s = 0;
    while s < n - p && s < m - p && base[n - 1 - s] == cur[m - 1 - s] {
        s += 1;
    }
    let bmid = &base[p..n - s];
    let cmid = &cur[p..m - s];

    if bmid.is_empty() && cmid.is_empty() {
        return out;
    }
    if bmid.is_empty() {
        out.push(Hunk { cur_lo: p, cur_hi: p + cmid.len(), base: Vec::new() });
        return out;
    }
    if cmid.is_empty() {
        let at = p.min(m.saturating_sub(1));
        out.push(Hunk { cur_lo: at, cur_hi: at, base: bmid.to_vec() });
        return out;
    }
    if bmid.len().max(cmid.len()) > 2000 {
        out.push(Hunk { cur_lo: p, cur_hi: p + cmid.len(), base: bmid.to_vec() });
        return out;
    }

    // LCS DP over the middle, then backtrack into an edit script.
    let (a, b) = (bmid, cmid);
    let (la, lb) = (a.len(), b.len());
    let w = lb + 1;
    let mut dp = vec![0u32; (la + 1) * w];
    for i in (0..la).rev() {
        for j in (0..lb).rev() {
            dp[i * w + j] = if a[i] == b[j] {
                dp[(i + 1) * w + j + 1] + 1
            } else {
                dp[(i + 1) * w + j].max(dp[i * w + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0usize, 0usize);
    let mut ci = 0usize; // current-line index within `cmid`
    while i < la || j < lb {
        if i < la && j < lb && a[i] == b[j] {
            i += 1;
            j += 1;
            ci += 1;
            continue;
        }
        let i0 = i;
        let (mut dels, mut ins) = (0usize, 0usize);
        while i < la || j < lb {
            if i < la && j < lb && a[i] == b[j] {
                break;
            }
            if j < lb && (i >= la || dp[i * w + j + 1] >= dp[(i + 1) * w + j]) {
                ins += 1;
                j += 1;
            } else if i < la {
                dels += 1;
                i += 1;
            } else {
                break;
            }
        }
        let base_lines = a[i0..i0 + dels].to_vec();
        out.push(Hunk { cur_lo: p + ci, cur_hi: p + ci + ins, base: base_lines });
        ci += ins;
    }
    out
}
