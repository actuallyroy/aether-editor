// Commit-graph layout for the "Visualize Repository History" view.
//
// Takes `git log` records (commit + parents, newest-first in date order) and
// assigns each commit a lane (column), producing per-row line segments that the
// renderer draws as the classic right-angle commit graph (axis-aligned quads, so
// shifts/merges/branches render as vertical stubs joined by a horizontal at the
// node's mid-line). Colours are assigned per lane and kept stable while the lane
// lives.

use crate::git::LogEntry;

/// Vertical extent of a lane segment within a row's band.
#[derive(Clone, Copy, PartialEq)]
pub enum Half {
    Full,
    Top,
    Bottom,
}

/// A graph line segment in one row's band: a vertical at a column (full / top-half
/// / bottom-half) or a horizontal at the mid-line joining two columns.
pub enum Seg {
    V { col: u16, half: Half, color: u8 },
    H { a: u16, b: u16, color: u8 },
    /// A curved connector between the row's node column and lane `col`, in the top
    /// half (a merge joining the node from above) or bottom half (a branch leaving
    /// the node below). The renderer draws it as a quarter-arc + straight stubs.
    Bend { col: u16, top: bool, color: u8 },
}

#[derive(Clone)]
pub enum RefKind {
    Head,
    Branch,
    Remote,
    Tag,
}

#[derive(Clone)]
pub struct Ref {
    pub label: String,
    pub kind: RefKind,
}

pub struct Row {
    pub node_col: u16,
    pub color: u8,
    pub segs: Vec<Seg>,
    pub short: String,
    pub refs: Vec<Ref>,
    pub author: String,
    pub when: String,
    pub subject: String,
    pub message: String, // full commit message (subject + body) for the hover tooltip
}

pub struct Graph {
    pub title: String,
    pub rows: Vec<Row>,
    pub max_col: u16, // widest lane index used (for laying out the text column)
}

/// Number of distinct lane colours we cycle through.
pub const PALETTE: u8 = 8;

/// Quads for a curved `Bend` connector between the node column (`node_cx`) and a
/// lane (`lane_cx`), in the top half (merge joining from above) or bottom half
/// (branch leaving below). A straight stub + quarter-arc + straight stub, so the
/// turn reads as a smooth curve. `lw` = line width; band spans `[top_y, bot_y]`,
/// node at `mid`.
pub fn bend_quads(node_cx: f32, lane_cx: f32, top_y: f32, bot_y: f32, mid: f32, top: bool, lw: f32, color: [f32; 4]) -> Vec<crate::quad::Quad> {
    use crate::quad::Quad;
    let half = lw * 0.5;
    let dx = lane_cx - node_cx;
    let r = dx.abs().min((mid - top_y).abs()).max(1.0); // arc radius
    let mut out = Vec::new();
    if top {
        // Lane comes down at lane_cx, curves toward the node, then a horizontal to it.
        let s = (node_cx - lane_cx).signum(); // toward the node
        out.push(Quad::new(lane_cx - half, top_y, lw, (mid - r - top_y).max(0.0), color));
        let (e1, e2) = ((lane_cx, mid - r), (lane_cx + s * r, mid)); // arc endpoints
        let c = (lane_cx + s * r, mid - r); // arc center
        let (minx, miny) = (e1.0.min(e2.0), e1.1.min(e2.1));
        out.push(Quad::arc(minx - half, miny - half, r + lw, r + lw, color, r + half, r - half, c.0, c.1));
        let (hx0, hx1) = ((lane_cx + s * r).min(node_cx), (lane_cx + s * r).max(node_cx));
        out.push(Quad::new(hx0, mid - half, hx1 - hx0, lw, color));
    } else {
        // Horizontal from the node, curve down, then a vertical down the lane.
        let s = dx.signum(); // toward the lane
        let (hx0, hx1) = (node_cx.min(lane_cx - s * r), node_cx.max(lane_cx - s * r));
        out.push(Quad::new(hx0, mid - half, hx1 - hx0, lw, color));
        let (e1, e2) = ((lane_cx - s * r, mid), (lane_cx, mid + r));
        let c = (lane_cx - s * r, mid + r);
        let (minx, miny) = (e1.0.min(e2.0), e1.1.min(e2.1));
        out.push(Quad::arc(minx - half, miny - half, r + lw, r + lw, color, r + half, r - half, c.0, c.1));
        out.push(Quad::new(lane_cx - half, mid + r, lw, (bot_y - (mid + r)).max(0.0), color));
    }
    out
}

fn parse_ref(d: &str) -> Ref {
    if let Some(rest) = d.strip_prefix("HEAD -> ") {
        return Ref { label: rest.to_string(), kind: RefKind::Head };
    }
    if d == "HEAD" {
        return Ref { label: "HEAD".to_string(), kind: RefKind::Head };
    }
    if let Some(t) = d.strip_prefix("tag: ") {
        return Ref { label: t.to_string(), kind: RefKind::Tag };
    }
    if d.contains('/') {
        return Ref { label: d.to_string(), kind: RefKind::Remote };
    }
    Ref { label: d.to_string(), kind: RefKind::Branch }
}

/// Relative "time ago" string from a unix timestamp, given `now` (unix seconds).
fn rel_time(ts: i64, now: i64) -> String {
    let d = (now - ts).max(0);
    let (n, unit) = if d < 60 {
        (d, "second")
    } else if d < 3600 {
        (d / 60, "minute")
    } else if d < 86_400 {
        (d / 3600, "hour")
    } else if d < 2_592_000 {
        (d / 86_400, "day")
    } else if d < 31_536_000 {
        (d / 2_592_000, "month")
    } else {
        (d / 31_536_000, "year")
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

/// First free lane index (a `None` slot), extending the vec if all are taken.
fn free_slot(lanes: &mut Vec<Option<String>>, colors: &mut Vec<u8>) -> usize {
    if let Some(i) = lanes.iter().position(|l| l.is_none()) {
        i
    } else {
        lanes.push(None);
        colors.push(0);
        lanes.len() - 1
    }
}

/// Build the laid-out graph from `git log` entries (newest-first). `now` is the
/// current unix time for relative dates (passed in — the renderer has no clock).
pub fn build(title: String, entries: Vec<LogEntry>, now: i64) -> Graph {
    let mut lanes: Vec<Option<String>> = Vec::new(); // hash each lane currently expects
    let mut colors: Vec<u8> = Vec::new(); // colour per lane (parallel to `lanes`)
    let mut next_color: u8 = 0;
    let mut rows = Vec::new();
    let mut max_col = 0u16;

    for e in &entries {
        let before = lanes.clone();
        let before_colors = colors.clone();

        // Node lane: the lane already expecting this commit, else a fresh one.
        let node_col = match before.iter().position(|l| l.as_deref() == Some(e.hash.as_str())) {
            Some(c) => c,
            None => {
                let c = free_slot(&mut lanes, &mut colors);
                colors[c] = next_color;
                next_color = (next_color + 1) % PALETTE;
                c
            }
        };
        let node_color = colors[node_col];

        // Build the post-commit lane state: clear every lane that expected this
        // commit (merges collapse into the node), then route parents.
        for (i, l) in lanes.iter_mut().enumerate() {
            if l.as_deref() == Some(e.hash.as_str()) {
                *l = None;
                let _ = i;
            }
        }
        let mut parent_cols: Vec<(usize, u8)> = Vec::new();
        for (k, p) in e.parents.iter().enumerate() {
            if let Some(c) = lanes.iter().position(|l| l.as_deref() == Some(p.as_str())) {
                parent_cols.push((c, colors[c])); // a lane already expects this parent → merge
            } else if k == 0 {
                lanes[node_col] = Some(p.clone());
                colors[node_col] = node_color; // first parent stays in the node's lane + colour
                parent_cols.push((node_col, node_color));
            } else {
                let c = free_slot(&mut lanes, &mut colors);
                lanes[c] = Some(p.clone());
                colors[c] = next_color;
                next_color = (next_color + 1) % PALETTE;
                parent_cols.push((c, colors[c]));
            }
        }
        // If the node's lane wasn't claimed by a parent, it ends here.
        if !parent_cols.iter().any(|(c, _)| *c == node_col) {
            lanes[node_col] = None;
        }
        // Trim trailing empty lanes so the graph doesn't grow forever.
        while lanes.last() == Some(&None) {
            lanes.pop();
            colors.pop();
        }

        // ---- Segments for this row's band ----
        let mut segs = Vec::new();
        // Continuing / merging incoming lanes.
        for (c, l) in before.iter().enumerate() {
            let Some(h) = l else { continue };
            let color = before_colors[c];
            if h == &e.hash {
                // Merges into the node from above — a curved connector (or straight V
                // when it's already the node's column).
                if c as u16 == node_col as u16 {
                    segs.push(Seg::V { col: c as u16, half: Half::Top, color });
                } else {
                    segs.push(Seg::Bend { col: c as u16, top: true, color });
                }
            } else if let Some(c2) = lanes.iter().position(|x| x.as_deref() == Some(h.as_str())) {
                // Lane survives; straight if same column, else a jog through the mid.
                if c2 == c {
                    segs.push(Seg::V { col: c as u16, half: Half::Full, color });
                } else {
                    segs.push(Seg::V { col: c as u16, half: Half::Top, color });
                    segs.push(Seg::H { a: c as u16, b: c2 as u16, color });
                    segs.push(Seg::V { col: c2 as u16, half: Half::Bottom, color });
                }
            }
        }
        // Outgoing parent lanes. Always connect the node to each parent's lane (so a
        // merge whose second parent already has a lane still shows the join); only
        // start a fresh bottom-half vertical for a newly-created lane (an existing
        // lane already draws its own full/jog vertical in the incoming pass).
        for (c, color) in &parent_cols {
            if *c as u16 != node_col as u16 {
                // Branch leaving the node below → curved connector (its lower vertical
                // stub is part of the bend; an existing lane's full V is already drawn).
                segs.push(Seg::Bend { col: *c as u16, top: false, color: *color });
            } else {
                segs.push(Seg::V { col: *c as u16, half: Half::Bottom, color: *color });
            }
        }

        max_col = max_col.max(lanes.len() as u16).max(node_col as u16 + 1);
        rows.push(Row {
            node_col: node_col as u16,
            color: node_color,
            segs,
            short: e.hash.chars().take(7).collect(),
            refs: e.refs.iter().map(|d| parse_ref(d)).collect(),
            author: e.author.clone(),
            when: rel_time(e.timestamp, now),
            subject: e.subject.clone(),
            message: if e.body.is_empty() {
                e.subject.clone()
            } else {
                format!("{}\n\n{}", e.subject, e.body)
            },
        });
    }

    Graph { title, rows, max_col }
}
