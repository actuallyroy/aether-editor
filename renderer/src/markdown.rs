// Markdown rendering for the README pane. Parses CommonMark/GFM with
// pulldown-cmark into a sequence of *blocks* — runs of inline text (each its own
// shaped, wrapping glyphon buffer) interleaved with block-level images. The block
// model lets images reserve real vertical space and report their on-screen rect so
// the media layer can draw the actual picture/GIF at that position. Inline styles
// (headings, code, links, quotes, lists) map to fonts + theme colors.

use glyphon::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, Style as FontStyle, TextArea, TextBounds, Weight};
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::theme;
use crate::quad::Quad;
use crate::widgets::Rect;

fn attrs(family: &'static str, color: Color) -> Attrs<'static> {
    Attrs::new().family(Family::Name(family)).color(color)
}

fn heading_metrics(level: HeadingLevel) -> Metrics {
    // Scale with the UI zoom, like body text (theme::UI_FONT_SIZE already does), so
    // headings don't stay tiny at high zoom.
    let z = theme::ui_zoom();
    match level {
        HeadingLevel::H1 => Metrics::new(22.0 * z, 30.0 * z),
        HeadingLevel::H2 => Metrics::new(18.0 * z, 26.0 * z),
        HeadingLevel::H3 => Metrics::new(16.0 * z, 24.0 * z),
        _ => Metrics::new(14.5 * z, 22.0 * z),
    }
}

struct Style {
    heading: Option<HeadingLevel>,
    code: bool,
    link: bool,
    quote: bool,
    bold: u32,   // **strong** nesting depth
    italic: u32, // *emphasis* nesting depth
}

impl Style {
    fn new() -> Self {
        Self { heading: None, code: false, link: false, quote: false, bold: 0, italic: 0 }
    }
    fn text_attrs(&self) -> Attrs<'static> {
        // Base color/family by block role, then layer inline weight/slant on top.
        let mut a = if self.heading.is_some() {
            // Headings are bold; size comes from the block's base metrics.
            attrs(theme::UI_FAMILY(), theme::MD_HEADING()).weight(Weight::BOLD)
        } else if self.code {
            attrs(theme::MONO_FAMILY(), theme::MD_CODE())
        } else if self.link {
            attrs(theme::UI_FAMILY(), theme::FG_ACTIVE())
        } else if self.quote {
            attrs(theme::UI_FAMILY(), theme::MD_QUOTE()).style(FontStyle::Italic)
        } else {
            attrs(theme::UI_FAMILY(), theme::FG_TEXT())
        };
        if self.bold > 0 {
            a = a.weight(Weight::BOLD);
        }
        if self.italic > 0 {
            a = a.style(FontStyle::Italic);
        }
        a
    }
}

/// A hyperlink within a text block: byte range [start, end) into the block's text
/// and the destination URL.
type LinkRun = (usize, usize, String);

enum Block {
    Text { buffer: Buffer, height: f32, links: Vec<LinkRun>, gap: f32 },
    Image { url: String },
    // A real grid table: each cell is its own shaped buffer (proportional font,
    // wrapped to its column width). `text_x`/`text_top` position cell text; the
    // `col_edge`/`row_edge` arrays are the vertical/horizontal grid-line positions
    // (drawn as thin quads in `collect_quads`). The header row (0) is bold.
    Table {
        cells: Vec<Vec<Buffer>>,
        text_x: Vec<f32>,    // per-column text left offset (rel. to table left)
        text_top: Vec<f32>,  // per-row text top offset (rel. to table top)
        col_edge: Vec<f32>,  // ncols+1 vertical grid-line x offsets
        row_edge: Vec<f32>,  // nrows+1 horizontal grid-line y offsets
        table_w: f32,
        height: f32,
        gap: f32,
    },
}

/// Lay out buffered table rows into a real grid table. Columns are sized
/// proportionally to content but never narrower than their longest word (so words
/// never break mid-letter), shrinking the slack to fit the pane. Cells wrap within
/// their column; each row is as tall as its tallest cell. The first row is the bold
/// header. Grid-line positions are recorded for `collect_quads` to stroke.
fn build_table(rows: &[Vec<String>], fs: &mut FontSystem, base_m: Metrics, avail: f32) -> Block {
    let z = theme::ui_zoom();
    let pad_x = 10.0 * z; // inner cell horizontal padding (each side)
    let pad_y = 6.0 * z;  // inner cell vertical padding (each side)
    let line = theme::zpx(1.0).max(1.0); // grid-line thickness
    let char_w = theme::UI_FONT_SIZE() * 0.55; // rough proportional advance
    let ncols = rows.iter().map(|r| r.len()).max().unwrap_or(1).max(1);

    // Per column: weight = widest cell (chars); word_min = longest single word.
    let mut weights = vec![1f32; ncols];
    let mut word_min = vec![1f32; ncols];
    for row in rows {
        for i in 0..ncols {
            let s = row.get(i).map(|s| s.trim()).unwrap_or("");
            weights[i] = weights[i].max(s.chars().count().max(1) as f32);
            let longest = s.split_whitespace().map(|w| w.chars().count()).max().unwrap_or(1).max(1);
            word_min[i] = word_min[i].max(longest as f32);
        }
    }
    let sumw: f32 = weights.iter().sum();
    // Width available for text after padding + grid lines.
    let chrome = (pad_x * 2.0) * ncols as f32 + line * (ncols as f32 + 1.0);
    let avail_text = (avail - chrome).max(ncols as f32 * 24.0);
    let mins: Vec<f32> = word_min.iter().map(|w| (w * char_w).max(24.0 * z)).collect();
    let mut col_w: Vec<f32> =
        (0..ncols).map(|i| (avail_text * (weights[i] / sumw)).max(mins[i])).collect();
    // Shrink slack-above-min proportionally if we overflowed the pane.
    // The preview has no horizontal scroll, so the table MUST fit the pane width —
    // never overflow (which would clip the rightmost columns unreadably).
    let total: f32 = col_w.iter().sum();
    if total > avail_text {
        let min_total: f32 = mins.iter().sum();
        if avail_text > min_total {
            // Shrink each column's slack-above-its-word-min proportionally.
            let factor = (avail_text - min_total) / (total - min_total);
            for i in 0..ncols {
                col_w[i] = mins[i] + (col_w[i] - mins[i]) * factor;
            }
        } else {
            // Even the word-minimums don't fit: scale them down to the pane. The
            // longest words may now wrap, but nothing is clipped off-screen.
            let f = avail_text / min_total;
            for i in 0..ncols {
                col_w[i] = mins[i] * f;
            }
        }
    }

    // Column edges + text x. A vertical line sits at each col_edge.
    let mut col_edge = vec![0.0f32; ncols + 1];
    let mut text_x = vec![0.0f32; ncols];
    let mut x = 0.0;
    for i in 0..ncols {
        col_edge[i] = x;
        text_x[i] = x + line + pad_x;
        x += line + pad_x + col_w[i] + pad_x;
    }
    col_edge[ncols] = x;
    let table_w = x + line;

    // Shape cells row by row, tracking row heights -> row edges + text tops.
    let mut cells: Vec<Vec<Buffer>> = Vec::with_capacity(rows.len());
    let mut row_edge = vec![0.0f32; rows.len() + 1];
    let mut text_top = vec![0.0f32; rows.len()];
    let mut y = 0.0f32;
    for (ri, row) in rows.iter().enumerate() {
        let mut rb = Vec::with_capacity(ncols);
        let mut h = base_m.line_height;
        for (i, cw) in col_w.iter().enumerate() {
            let text = row.get(i).map(|s| s.trim()).unwrap_or("");
            let a = if ri == 0 {
                attrs(theme::UI_FAMILY(), theme::MD_HEADING()).weight(Weight::BOLD)
            } else {
                attrs(theme::UI_FAMILY(), theme::FG_TEXT())
            };
            let mut buf = Buffer::new(fs, base_m);
            buf.set_size(fs, Some(*cw), Some(100_000.0));
            buf.set_rich_text(fs, std::iter::once((text, a)), a, Shaping::Advanced);
            buf.shape_until_scroll(fs, false);
            let mut vlines = 0usize;
            for li in 0..buf.lines.len() {
                if let Some(l) = buf.line_layout(fs, li) {
                    vlines += l.len();
                }
            }
            h = h.max(vlines as f32 * base_m.line_height);
            rb.push(buf);
        }
        row_edge[ri] = y;
        text_top[ri] = y + line + pad_y;
        y += line + pad_y + h + pad_y;
        cells.push(rb);
    }
    row_edge[rows.len()] = y;
    let height = y + line;

    Block::Table { cells, text_x, text_top, col_edge, row_edge, table_w, height, gap: block_gap() }
}

pub struct Markdown {
    blocks: Vec<Block>,
    image_urls: Vec<String>,
    last_key: String,
    width: f32,
}

// Image/block spacing + caps, scaled with the UI zoom so they track the text size.
fn img_gap() -> f32 { 10.0 * theme::ui_zoom() }
fn img_placeholder_h() -> f32 { 160.0 * theme::ui_zoom() }
fn img_max_h() -> f32 { 420.0 * theme::ui_zoom() }
// Vertical rhythm between blocks (scaled with UI zoom). Bigger gaps above headings
// and between paragraphs give the preview breathing room instead of cramming lines.
fn block_gap() -> f32 { 10.0 * theme::ui_zoom() } // default between stacked blocks
fn para_gap() -> f32 { 12.0 * theme::ui_zoom() }  // after a paragraph
fn head_top_gap() -> f32 { 22.0 * theme::ui_zoom() } // before a heading (space above)
fn head_bot_gap() -> f32 { 8.0 * theme::ui_zoom() }  // after a heading
fn list_gap() -> f32 { 5.0 * theme::ui_zoom() }   // between list items

impl Markdown {
    pub fn new(_fs: &mut FontSystem) -> Self {
        Self { blocks: Vec::new(), image_urls: Vec::new(), last_key: String::new(), width: 0.0 }
    }

    /// Re-parse + reshape only when content (`key`) or wrap `width` changes.
    pub fn set(&mut self, fs: &mut FontSystem, key: &str, src: &str, width: f32) {
        if self.last_key == key && (self.width - width).abs() < 0.5 {
            return;
        }
        self.blocks.clear();
        self.image_urls.clear();

        let mut opts = Options::empty();
        opts.insert(Options::ENABLE_STRIKETHROUGH);
        opts.insert(Options::ENABLE_TABLES);
        opts.insert(Options::ENABLE_TASKLISTS);
        let parser = Parser::new_ext(src, opts);

        let base = attrs(theme::UI_FAMILY(), theme::FG_TEXT());
        let mut spans: Vec<(String, Attrs<'static>)> = Vec::new();
        let mut st = Style::new();
        let mut list_stack: Vec<Option<u64>> = Vec::new();
        let mut image_depth = 0u32; // >0 while inside an image (skip its alt text)
        // Tables are buffered (rows of plain-cell text) and rendered column-aligned in
        // a monospace block on TagEnd::Table, instead of inline run-together cells.
        let mut in_table = false;
        let mut table_rows: Vec<Vec<String>> = Vec::new();

        // Flush accumulated inline spans into a Text block shaped at `metrics`
        // (the block's uniform line advance — headings use a larger metrics so
        // consecutive heading lines don't overlap, which a per-span override can't
        // fix since cosmic-text advances by the buffer's base line height).
        let mut flush = |spans: &mut Vec<(String, Attrs<'static>)>, blocks: &mut Vec<Block>, fs: &mut FontSystem, metrics: Metrics, links: &mut Vec<LinkRun>, gap: f32| {
            if spans.iter().all(|(s, _)| s.trim().is_empty()) {
                spans.clear();
                links.clear();
                return;
            }
            let mut buffer = Buffer::new(fs, metrics);
            buffer.set_size(fs, Some(width), Some(100_000.0));
            buffer.set_rich_text(fs, spans.iter().map(|(s, a)| (s.as_str(), *a)), base, Shaping::Advanced);
            buffer.shape_until_scroll(fs, false);
            // Force layout of EVERY logical line and count the resulting visual
            // (wrapped) lines. shape_until_scroll only lays out the first screenful,
            // so measuring via layout_runs under-reports tall blocks' height — which
            // made later blocks stack on top of them. line_layout forces full layout.
            let mut visual_lines = 0usize;
            for i in 0..buffer.lines.len() {
                if let Some(layout) = buffer.line_layout(fs, i) {
                    visual_lines += layout.len();
                }
            }
            let height = visual_lines as f32 * metrics.line_height;
            blocks.push(Block::Text { buffer, height, links: std::mem::take(links), gap });
            spans.clear();
        };
        // Roomier line height than the editor's so prose breathes (~1.5x font size).
        let base_m = Metrics::new(theme::UI_FONT_SIZE(), (theme::UI_FONT_SIZE() * 1.5).max(theme::UI_LINE_HEIGHT()));
        // Link tracking: byte offset within the current block's accumulated text.
        let byte_len = |spans: &[(String, Attrs<'static>)]| spans.iter().map(|(s, _)| s.len()).sum::<usize>();
        let mut cur_links: Vec<LinkRun> = Vec::new();
        let mut link_open: Option<(usize, String)> = None;

        for ev in parser {
            match ev {
                Event::Start(tag) => match tag {
                    Tag::Heading { level, .. } => {
                        // Headings get their own uniform-metrics block. Give the block
                        // just above the heading extra trailing space (space above it).
                        flush(&mut spans, &mut self.blocks, fs, base_m, &mut cur_links, head_top_gap());
                        if let Some(Block::Text { gap, .. }) = self.blocks.last_mut() {
                            *gap = gap.max(head_top_gap());
                        }
                        st.heading = Some(level);
                    }
                    Tag::CodeBlock(_) => {
                        st.code = true;
                        spans.push(("\n".into(), base));
                    }
                    Tag::Link { dest_url, .. } => {
                        st.link = true;
                        link_open = Some((byte_len(&spans), dest_url.to_string()));
                    }
                    Tag::Image { dest_url, .. } => {
                        image_depth += 1;
                        // Block-level image. Skip SVGs (no rasterizer) so we don't
                        // reserve empty placeholder gaps for shields-style badges.
                        let url = dest_url.to_string();
                        let is_svg = url.split('?').next().unwrap_or(&url).to_lowercase().ends_with(".svg");
                        if !url.is_empty() && !is_svg {
                            flush(&mut spans, &mut self.blocks, fs, base_m, &mut cur_links, block_gap());
                            self.image_urls.push(url.clone());
                            self.blocks.push(Block::Image { url });
                        }
                    }
                    Tag::List(start) => list_stack.push(start),
                    Tag::Item => {
                        let indent = "    ".repeat(list_stack.len().saturating_sub(1));
                        let marker = match list_stack.last_mut() {
                            Some(Some(n)) => { let s = format!("{n}. "); *n += 1; s }
                            _ => "•  ".to_string(),
                        };
                        spans.push((format!("{indent}{marker}"), attrs(theme::UI_FAMILY(), theme::MD_LIST())));
                    }
                    Tag::BlockQuote(_) => st.quote = true,
                    // Tables: buffer cells, then render column-aligned (monospace) on end.
                    Tag::Table(_) => {
                        flush(&mut spans, &mut self.blocks, fs, base_m, &mut cur_links, block_gap());
                        in_table = true;
                        table_rows.clear();
                    }
                    Tag::TableHead | Tag::TableRow => table_rows.push(Vec::new()),
                    Tag::TableCell => {
                        if let Some(row) = table_rows.last_mut() {
                            row.push(String::new());
                        }
                    }
                    Tag::Strong => st.bold += 1,
                    Tag::Emphasis => st.italic += 1,
                    _ => {}
                },
                Event::End(tag) => match tag {
                    TagEnd::Heading(_) => {
                        let m = st.heading.map(heading_metrics).unwrap_or(base_m);
                        flush(&mut spans, &mut self.blocks, fs, m, &mut cur_links, head_bot_gap());
                        st.heading = None;
                    }
                    TagEnd::CodeBlock => { st.code = false; spans.push(("\n".into(), base)); }
                    TagEnd::Link => {
                        st.link = false;
                        if let Some((s, url)) = link_open.take() {
                            let e = byte_len(&spans);
                            if e > s {
                                cur_links.push((s, e, url));
                            }
                        }
                    }
                    TagEnd::Image => image_depth = image_depth.saturating_sub(1),
                    TagEnd::List(_) => { list_stack.pop(); if list_stack.is_empty() { spans.push(("\n".into(), base)); } }
                    // Flush each item as its own block so the inter-block gap separates
                    // bullets (a single '\n' packed them with no breathing room).
                    TagEnd::Item => flush(&mut spans, &mut self.blocks, fs, base_m, &mut cur_links, list_gap()),
                    TagEnd::BlockQuote(_) => { st.quote = false; spans.push(("\n".into(), base)); }
                    TagEnd::Paragraph => spans.push(("\n\n".into(), base)),
                    TagEnd::TableHead | TagEnd::TableRow => {}
                    TagEnd::Table => {
                        in_table = false;
                        if !table_rows.is_empty() {
                            let tbl = build_table(&table_rows, fs, base_m, width);
                            self.blocks.push(tbl);
                        }
                        table_rows.clear();
                    }
                    TagEnd::Strong => st.bold = st.bold.saturating_sub(1),
                    TagEnd::Emphasis => st.italic = st.italic.saturating_sub(1),
                    _ => {}
                },
                Event::Text(t) => {
                    if in_table {
                        if let Some(c) = table_rows.last_mut().and_then(|r| r.last_mut()) {
                            c.push_str(&t);
                        }
                    } else if image_depth == 0 {
                        spans.push((t.to_string(), st.text_attrs()));
                    }
                }
                Event::Code(t) if in_table => {
                    if let Some(c) = table_rows.last_mut().and_then(|r| r.last_mut()) {
                        c.push_str(&t);
                    }
                }
                Event::Code(t) => spans.push((t.to_string(), attrs(theme::MONO_FAMILY(), theme::MD_CODE()))),
                Event::SoftBreak => spans.push((" ".into(), base)),
                Event::HardBreak => spans.push(("\n".into(), base)),
                Event::Rule => spans.push(("\n────────────────\n\n".into(), attrs(theme::UI_FAMILY(), theme::MD_RULE()))),
                Event::TaskListMarker(done) => spans.push(((if done { "[x] " } else { "[ ] " }).into(), st.text_attrs())),
                _ => {}
            }
        }
        flush(&mut spans, &mut self.blocks, fs, base_m, &mut cur_links, block_gap());

        self.last_key = key.to_string();
        self.width = width;
    }

    /// All image URLs referenced (for prefetching).
    pub fn image_urls(&self) -> &[String] {
        &self.image_urls
    }

    /// Display height of an image given its natural size and the column width.
    fn image_height(natural: Option<(f32, f32)>, width: f32) -> f32 {
        match natural {
            Some((w, h)) if w > 0.0 => {
                let dw = w.min(width);
                (dw * h / w).min(img_max_h())
            }
            _ => img_placeholder_h(),
        }
    }

    /// Total laid-out height (depends on loaded image sizes via `size_of`).
    pub fn content_height(&self, size_of: &dyn Fn(&str) -> Option<(f32, f32)>) -> f32 {
        let mut y = 0.0;
        for b in &self.blocks {
            match b {
                Block::Text { height, gap, .. } => y += height + gap,
                Block::Table { height, gap, .. } => y += height + gap,
                // Only loaded images take space — unloaded/failed ones collapse to
                // nothing (no empty gap) until their pixels arrive.
                Block::Image { url } => {
                    if let Some(nat) = size_of(url) {
                        y += Self::image_height(Some(nat), self.width) + img_gap() * 2.0;
                    }
                }
            }
        }
        y
    }

    /// Grid-line quads for any tables, in the same layout `draw` uses. Called in the
    /// render's quad phase (quads are drawn behind text), so table cells sit inside a
    /// real ruled grid. Lines are clamped to the viewport so scrolled-off tables don't
    /// bleed over other UI.
    pub fn collect_quads(&self, rect: Rect, scroll: f32, size_of: &dyn Fn(&str) -> Option<(f32, f32)>) -> Vec<Quad> {
        let line = theme::zpx(1.0).max(1.0);
        let col = theme::PANEL_BORDER();
        let top_clip = rect.y;
        let bot_clip = rect.y + rect.h;
        let mut quads = Vec::new();
        let mut y = rect.y - scroll;
        for b in &self.blocks {
            match b {
                Block::Text { height, gap, .. } => y += height + gap,
                Block::Image { url } => {
                    if let Some(nat) = size_of(url) {
                        y += Self::image_height(Some(nat), self.width) + img_gap() * 2.0;
                    }
                }
                Block::Table { col_edge, row_edge, table_w, height, gap, .. } => {
                    if y + height > top_clip && y < bot_clip {
                        // Vertical lines (clamped vertically to the viewport).
                        let vt = (y).max(top_clip);
                        let vb = (y + height).min(bot_clip);
                        if vb > vt {
                            for ex in col_edge {
                                quads.push(Quad::new(rect.x + ex, vt, line, vb - vt, col));
                            }
                        }
                        // Horizontal lines (only those in view).
                        for ey in row_edge {
                            let ly = y + ey;
                            if ly >= top_clip - line && ly <= bot_clip {
                                quads.push(Quad::new(rect.x, ly, *table_w, line, col));
                            }
                        }
                    }
                    y += height + gap;
                }
            }
        }
        quads
    }

    /// Draw text blocks (clipped + scrolled) and collect image draw rects for the
    /// media layer. `size_of` supplies loaded image natural sizes.
    pub fn draw<'a>(
        &'a self,
        rect: Rect,
        scroll: f32,
        size_of: &dyn Fn(&str) -> Option<(f32, f32)>,
        areas: &mut Vec<TextArea<'a>>,
        img_rects: &mut Vec<(String, Rect)>,
    ) {
        let clip = TextBounds {
            left: rect.x as i32,
            top: rect.y as i32,
            right: (rect.x + rect.w) as i32,
            bottom: (rect.y + rect.h) as i32,
        };
        let mut y = rect.y - scroll;
        for b in &self.blocks {
            match b {
                Block::Text { buffer, height, gap, .. } => {
                    // Only emit if any part is within the viewport.
                    if y + height > rect.y && y < rect.y + rect.h {
                        areas.push(TextArea {
                            buffer,
                            left: rect.x,
                            top: y,
                            scale: 1.0,
                            bounds: clip,
                            default_color: theme::FG_TEXT(),
                            custom_glyphs: &[],
                        });
                    }
                    y += height + gap;
                }
                Block::Table { cells, text_x, text_top, height, gap, .. } => {
                    if y + height > rect.y && y < rect.y + rect.h {
                        for (r, row) in cells.iter().enumerate() {
                            let ry = y + text_top[r];
                            for (c, buf) in row.iter().enumerate() {
                                areas.push(TextArea {
                                    buffer: buf,
                                    left: rect.x + text_x[c],
                                    top: ry,
                                    scale: 1.0,
                                    bounds: clip,
                                    default_color: theme::FG_TEXT(),
                                    custom_glyphs: &[],
                                });
                            }
                        }
                    }
                    y += height + gap;
                }
                Block::Image { url } => {
                    // Only loaded images occupy space + draw; unloaded ones collapse.
                    if let Some((nw, nh)) = size_of(url) {
                        let dh = Self::image_height(Some((nw, nh)), self.width);
                        let dw = if nw > 0.0 { nw.min(self.width) } else { self.width };
                        y += img_gap();
                        if y + dh > rect.y && y < rect.y + rect.h {
                            img_rects.push((url.clone(), Rect { x: rect.x, y, w: dw, h: dh }));
                        }
                        y += dh + img_gap();
                    }
                }
            }
        }
    }

    /// Screen-space rects for every visible link fragment (a link may span several
    /// runs/lines → several rects), each paired with its URL. Used both to draw
    /// underlines and to hit-test clicks — single source of truth for link geometry.
    pub fn link_geometry(&self, rect: Rect, scroll: f32, size_of: &dyn Fn(&str) -> Option<(f32, f32)>) -> Vec<(Rect, String)> {
        let mut out = Vec::new();
        let mut y = rect.y - scroll;
        for b in &self.blocks {
            match b {
                Block::Text { buffer, height, links, gap } => {
                    if !links.is_empty() && y + height > rect.y && y < rect.y + rect.h {
                        // Glyph `start` offsets are local to each logical line; build
                        // each line's start offset in the block's global text so we
                        // can match against the (global) link byte ranges.
                        let mut line_start: Vec<usize> = Vec::with_capacity(buffer.lines.len());
                        let mut acc = 0usize;
                        for bl in buffer.lines.iter() {
                            line_start.push(acc);
                            acc += bl.text().len() + 1; // +1 for the '\n' separator
                        }
                        for run in buffer.layout_runs() {
                            let base = line_start.get(run.line_i).copied().unwrap_or(0);
                            let line_y = y + run.line_top;
                            for (s, e, url) in links {
                                let mut lo = f32::INFINITY;
                                let mut hi = f32::NEG_INFINITY;
                                for g in run.glyphs.iter() {
                                    let gs = base + g.start;
                                    if gs >= *s && gs < *e {
                                        lo = lo.min(g.x);
                                        hi = hi.max(g.x + g.w);
                                    }
                                }
                                if hi > lo {
                                    out.push((
                                        Rect { x: rect.x + lo, y: line_y, w: hi - lo, h: run.line_height },
                                        url.clone(),
                                    ));
                                }
                            }
                        }
                    }
                    y += height + gap;
                }
                // Tables carry no links (cells are plain text); just advance past them.
                Block::Table { height, gap, .. } => y += height + gap,
                Block::Image { url } => {
                    if let Some(nat) = size_of(url) {
                        y += Self::image_height(Some(nat), self.width) + img_gap() * 2.0;
                    }
                }
            }
        }
        out
    }
}
