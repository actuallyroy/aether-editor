// Breadcrumb path bar below the tab strip. Each path segment is clickable: a
// folder segment opens a dropdown of that folder's contents (drill into subfolders,
// click a file to open it), like VSCode. Owns its own per-segment layout +
// hit-testing + the dropdown popup (single source of truth for draw and clicks).

use std::path::{Path, PathBuf};

use glyphon::{FontSystem, TextArea};

use crate::quad::Quad;
use crate::theme;
use crate::widgets::{Menu, Rect, TextLabel};

const SEP: &str = "  ›  ";

struct Crumb {
    path: PathBuf, // absolute path of this segment
    is_dir: bool,
    label: TextLabel,
}

pub struct Breadcrumbs {
    crumbs: Vec<Crumb>,
    sep: TextLabel,
    key: String, // change-detection for the current path
    /// Which segment's dropdown is open (None = closed).
    pub open: Option<usize>,
    /// Directory whose contents the dropdown currently lists.
    dropdown_dir: PathBuf,
    /// Dropdown entries: (name, absolute path, is_dir), folders first.
    entries: Vec<(String, PathBuf, bool)>,
    menu: Menu,
    pub hovered_row: Option<usize>,
}

impl Breadcrumbs {
    pub fn new(fs: &mut FontSystem) -> Self {
        let mut sep = TextLabel::new(fs, theme::zpx(40.0), theme::BREADCRUMB_HEIGHT());
        sep.set(fs, SEP, theme::UI_FAMILY());
        Self {
            crumbs: Vec::new(),
            sep,
            key: String::new(),
            open: None,
            dropdown_dir: PathBuf::new(),
            entries: Vec::new(),
            menu: Menu::new(fs, theme::zpx(320.0)),
            hovered_row: None,
        }
    }

    /// Rebuild the segments for `file` relative to workspace `cwd` (no-op if
    /// unchanged). Folders are every segment but the last (the file itself).
    pub fn set_path(&mut self, fs: &mut FontSystem, cwd: &Path, file: &Path) {
        let rel = file.strip_prefix(cwd).unwrap_or(file);
        let k = rel.to_string_lossy().into_owned();
        if k == self.key && !self.crumbs.is_empty() {
            // Re-shape labels on zoom change even when the path is unchanged.
            for c in &mut self.crumbs {
                c.label.reshape(fs);
            }
            self.sep.reshape(fs);
            return;
        }
        self.key = k;
        self.crumbs.clear();
        let parts: Vec<_> = rel.components().collect();
        let last = parts.len().saturating_sub(1);
        let mut acc = cwd.to_path_buf();
        for (i, comp) in parts.iter().enumerate() {
            acc.push(comp.as_os_str());
            let mut label = TextLabel::new(fs, theme::zpx(400.0), theme::BREADCRUMB_HEIGHT());
            label.set(fs, &comp.as_os_str().to_string_lossy(), theme::UI_FAMILY());
            self.crumbs.push(Crumb { path: acc.clone(), is_dir: i != last, label });
        }
    }

    /// The x-laid-out rect of each segment within the bar `r` (left-padded).
    pub fn segment_rects(&self, r: Rect) -> Vec<Rect> {
        let mut out = Vec::with_capacity(self.crumbs.len());
        let mut x = r.x + theme::zpx(14.0);
        let sep_w = self.sep.width();
        for (i, c) in self.crumbs.iter().enumerate() {
            let w = c.label.width();
            out.push(Rect { x, y: r.y, w, h: r.h });
            x += w;
            if i + 1 < self.crumbs.len() {
                x += sep_w;
            }
        }
        out
    }

    /// Segment index under `p` (for click / hover), if any.
    pub fn segment_at(&self, r: Rect, p: (f32, f32)) -> Option<usize> {
        self.segment_rects(r).iter().position(|seg| seg.contains(p))
    }

    pub fn draw<'a>(&'a self, r: Rect, hovered_seg: Option<usize>, areas: &mut Vec<TextArea<'a>>) {
        let rects = self.segment_rects(r);
        let sep_w = self.sep.width();
        for (i, c) in self.crumbs.iter().enumerate() {
            let seg = rects[i];
            let lit = hovered_seg == Some(i) || self.open == Some(i);
            let color = if i + 1 == self.crumbs.len() {
                theme::FG_TEXT() // the file itself
            } else if lit {
                theme::FG_TEXT()
            } else {
                theme::FG_DIM()
            };
            c.label.draw_left(seg, 0.0, color, areas);
            if i + 1 < self.crumbs.len() {
                let sep_rect = Rect { x: seg.x + seg.w, y: r.y, w: sep_w, h: r.h };
                self.sep.draw_left(sep_rect, 0.0, theme::FG_GUTTER(), areas);
            }
        }
    }

    // ---- dropdown ----

    /// Toggle the dropdown for segment `i`: lists that segment's folder (or, for the
    /// file segment, its parent), so you can jump to a sibling/child.
    pub fn toggle(&mut self, fs: &mut FontSystem, i: usize) {
        if self.open == Some(i) {
            self.close();
            return;
        }
        let Some(c) = self.crumbs.get(i) else { return };
        let dir = if c.is_dir { c.path.clone() } else { c.path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| c.path.clone()) };
        self.open = Some(i);
        self.load_dir(fs, dir);
    }

    /// Populate the dropdown with `dir`'s entries (folders first, then files).
    fn load_dir(&mut self, fs: &mut FontSystem, dir: PathBuf) {
        self.entries.clear();
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if matches!(name.as_str(), ".git" | ".DS_Store" | "Thumbs.db") {
                    continue;
                }
                let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                self.entries.push((name, e.path(), is_dir));
            }
        }
        self.entries.sort_by(|a, b| match (a.2, b.2) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.0.to_lowercase().cmp(&b.0.to_lowercase()),
        });
        let rows: Vec<(&str, &str, bool)> = self.entries.iter().map(|(n, _, _)| (n.as_str(), "", false)).collect();
        self.menu.set_entries(fs, &rows);
        self.dropdown_dir = dir;
    }

    pub fn close(&mut self) {
        self.open = None;
        self.hovered_row = None;
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// The dropdown popup rect, anchored under the open segment, clamped to `win`.
    pub fn dropdown_rect(&self, bar: Rect, win: (f32, f32)) -> Option<Rect> {
        let i = self.open?;
        let seg = *self.segment_rects(bar).get(i)?;
        Some(self.menu.rect((seg.x, bar.y + bar.h), win))
    }

    pub fn dropdown_item_at(&self, rect: Rect, p: (f32, f32)) -> Option<usize> {
        self.menu.item_at(rect, p)
    }

    /// The entry (name, path, is_dir) at dropdown row `i`.
    pub fn entry(&self, i: usize) -> Option<(PathBuf, bool)> {
        self.entries.get(i).map(|(_, p, d)| (p.clone(), *d))
    }

    /// Open the dropdown for a drilled-into folder `dir` (keeps the bar open).
    pub fn drill(&mut self, fs: &mut FontSystem, dir: PathBuf) {
        self.load_dir(fs, dir);
    }

    pub fn draw_dropdown_quads(&self, rect: Rect, quads: &mut Vec<Quad>) {
        self.menu.draw_bg(rect, self.hovered_row, quads);
    }

    pub fn draw_dropdown<'a>(&'a self, rect: Rect, areas: &mut Vec<TextArea<'a>>) {
        self.menu.draw(rect, areas);
    }
}
