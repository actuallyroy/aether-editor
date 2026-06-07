// Run & Debug sidebar panel. A toolbar (start/continue/step/stop/attach) plus an
// interactive tree: collapsible CALL STACK and VARIABLES sections, expandable
// scopes and structured variables (chevrons, indentation, hover, lazy-load on
// expand). State is pushed in by App from DAP `WorkerMsg` events; the panel emits
// `Intent`s for app-level actions (start/step/select-frame/expand-var).

use glyphon::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping, TextArea, TextBounds};

use crate::dap::{Scope, StackFrame, Variable};
use crate::quad::Quad;
use crate::theme;
use crate::ui::Intent;
use crate::widgets::{IconButton, Rect, ScrollOpts, ScrollView, TextLabel};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Session {
    Idle,
    Running,
    Stopped,
}

/// A structured variable in the tree (expandable when `var_ref > 0`).
struct VarNode {
    name: String,
    value: String,
    var_ref: i64,
    expanded: bool,
    loaded: bool,
    children: Vec<VarNode>,
}

/// A variable scope (Locals/Globals/…) for the stopped frame.
struct ScopeNode {
    name: String,
    var_ref: i64,
    expanded: bool,
    loaded: bool,
    vars: Vec<VarNode>,
}

/// What clicking a flattened row does.
#[derive(Clone)]
enum RowAct {
    ToggleCallStack,
    ToggleVariables,
    SelectFrame(i64),
    ToggleVar(i64), // by variablesReference (scope or var)
    None,
}

/// One flattened, visible row (rebuilt whenever the tree/expansion changes).
struct Row {
    act: RowAct,
}

pub struct DebugPanel {
    pub configs: Vec<crate::debug_config::LaunchConfig>,
    pub selected: usize,
    pub session: Session,
    config_label: TextLabel,
    hint: TextLabel,
    // Toolbar: 0 start/continue, 1 step-over, 2 step-in, 3 step-out, 4 stop,
    // 5 restart, 6 attach-to-process.
    btns: [IconButton; 7],
    btn_pause: IconButton,
    // Tree state.
    frames: Vec<StackFrame>,
    scopes: Vec<ScopeNode>,
    call_stack_open: bool,
    variables_open: bool,
    // Rendering: one rich-text buffer (chevron glyphs + labels) + per-row hit data.
    body: Buffer,
    rows: Vec<Row>,
    hovered: Option<usize>,
    scroll: ScrollView,
    last_w: f32,
}

const TOOLBAR_H: f32 = 36.0;
const BTN: f32 = 26.0;
const PAD: f32 = 8.0;
const TOP_PAD: f32 = 4.0;

impl DebugPanel {
    pub fn new(fs: &mut FontSystem) -> Self {
        let ic = theme::ICON_FAMILY;
        let mk = |fs: &mut FontSystem, g: char| IconButton::new(fs, g, ic, 16.0);
        let btns = [
            mk(fs, theme::ICON_DEBUG_START),
            mk(fs, theme::ICON_DEBUG_STEP_OVER),
            mk(fs, theme::ICON_DEBUG_STEP_INTO),
            mk(fs, theme::ICON_DEBUG_STEP_OUT),
            mk(fs, theme::ICON_DEBUG_STOP),
            mk(fs, theme::ICON_DEBUG_RESTART),
            mk(fs, theme::ICON_DEBUG_ATTACH),
        ];
        let btn_pause = mk(fs, theme::ICON_DEBUG_PAUSE);
        let mut body = Buffer::new(fs, Metrics::new(theme::UI_FONT_SIZE(), theme::UI_LINE_HEIGHT()));
        body.set_size(fs, Some(4000.0), Some(4000.0));
        Self {
            configs: Vec::new(),
            selected: 0,
            session: Session::Idle,
            config_label: TextLabel::new(fs, 600.0, theme::UI_LINE_HEIGHT() * 1.5),
            hint: TextLabel::new(fs, 4000.0, theme::UI_LINE_HEIGHT() * 1.5),
            btns,
            btn_pause,
            frames: Vec::new(),
            scopes: Vec::new(),
            call_stack_open: true,
            variables_open: true,
            body,
            rows: Vec::new(),
            hovered: None,
            scroll: ScrollView::new(ScrollOpts { vertical: true, horizontal: false, stick_to_end: false }),
            last_w: -1.0,
        }
    }

    pub fn reshape(&mut self, fs: &mut FontSystem) {
        for b in &mut self.btns {
            b.reshape(fs);
        }
        self.btn_pause.reshape(fs);
        self.last_w = -1.0;
        self.rebuild(fs);
    }

    pub fn set_configs(&mut self, fs: &mut FontSystem, configs: Vec<crate::debug_config::LaunchConfig>) {
        if self.selected >= configs.len() {
            self.selected = 0;
        }
        self.configs = configs;
        self.refresh_labels(fs);
    }

    fn refresh_labels(&mut self, fs: &mut FontSystem) {
        let name = self.configs.get(self.selected).map(|c| c.name.as_str()).unwrap_or("No Configurations");
        self.config_label.set(fs, name, theme::UI_FAMILY());
        let hint = match self.session {
            Session::Idle if self.configs.is_empty() => "Add a .vscode/launch.json, or open a file to debug.",
            Session::Idle => "Click ▷ to start debugging.",
            Session::Running => "Running… set a breakpoint in the gutter.",
            Session::Stopped => "",
        };
        self.hint.set(fs, hint, theme::UI_FAMILY());
    }

    pub fn set_session(&mut self, fs: &mut FontSystem, s: Session) {
        self.session = s;
        if s != Session::Stopped {
            self.frames.clear();
            self.scopes.clear();
        }
        self.refresh_labels(fs);
        self.rebuild(fs);
    }

    pub fn set_stopped(&mut self, fs: &mut FontSystem, frames: Vec<StackFrame>) {
        self.session = Session::Stopped;
        self.frames = frames;
        self.scopes.clear();
        self.call_stack_open = true;
        self.variables_open = true;
        self.refresh_labels(fs);
        self.rebuild(fs);
    }

    /// Set the scopes for the stopped frame (collapsed; App requests their vars).
    pub fn set_scopes(&mut self, fs: &mut FontSystem, scopes: Vec<Scope>) {
        self.scopes = scopes
            .into_iter()
            .enumerate()
            .map(|(i, s)| ScopeNode {
                name: s.name,
                var_ref: s.var_ref,
                expanded: i == 0, // expand the first scope (Locals) by default
                loaded: false,
                vars: Vec::new(),
            })
            .collect();
        self.rebuild(fs);
    }

    /// Scopes whose variables should be fetched now (expanded + not yet loaded).
    pub fn pending_scope_refs(&self) -> Vec<i64> {
        self.scopes.iter().filter(|s| s.expanded && !s.loaded).map(|s| s.var_ref).collect()
    }

    /// Fill the children of the scope or variable identified by `var_ref`.
    pub fn set_children(&mut self, fs: &mut FontSystem, var_ref: i64, vars: Vec<Variable>) {
        let kids: Vec<VarNode> = vars
            .into_iter()
            .map(|v| VarNode { name: v.name, value: v.value, var_ref: v.var_ref, expanded: false, loaded: false, children: Vec::new() })
            .collect();
        // Scope?
        for s in &mut self.scopes {
            if s.var_ref == var_ref {
                s.vars = kids;
                s.loaded = true;
                self.rebuild(fs);
                return;
            }
            if Self::fill_var(&mut s.vars, var_ref, &kids) {
                self.rebuild(fs);
                return;
            }
        }
    }

    fn fill_var(nodes: &mut [VarNode], var_ref: i64, kids: &[VarNode]) -> bool {
        for n in nodes {
            if n.var_ref == var_ref {
                n.children = kids.iter().map(|c| VarNode { name: c.name.clone(), value: c.value.clone(), var_ref: c.var_ref, expanded: false, loaded: false, children: Vec::new() }).collect();
                n.loaded = true;
                return true;
            }
            if Self::fill_var(&mut n.children, var_ref, kids) {
                return true;
            }
        }
        false
    }

    /// Toggle the expansion of the scope/var with `var_ref`. Returns Some(var_ref)
    /// if children must be lazily fetched.
    fn toggle_var(&mut self, var_ref: i64) -> Option<i64> {
        for s in &mut self.scopes {
            if s.var_ref == var_ref {
                s.expanded = !s.expanded;
                return (s.expanded && !s.loaded).then_some(var_ref);
            }
            if let Some(r) = Self::toggle_in(&mut s.vars, var_ref) {
                return r;
            }
        }
        None
    }

    fn toggle_in(nodes: &mut [VarNode], var_ref: i64) -> Option<Option<i64>> {
        for n in nodes {
            if n.var_ref == var_ref {
                n.expanded = !n.expanded;
                return Some((n.expanded && !n.loaded).then_some(var_ref));
            }
            if let Some(r) = Self::toggle_in(&mut n.children, var_ref) {
                return Some(r);
            }
        }
        None
    }

    /// Rebuild the flattened row list + the rich-text body buffer.
    fn rebuild(&mut self, fs: &mut FontSystem) {
        let dim = theme::FG_DIM();
        let fg = theme::FG_TEXT();
        let name_c = theme::SYN_VARIABLE();
        let icon = theme::ICON_FAMILY;
        let uifam = theme::UI_FAMILY();
        // Build spans (chevron glyphs in codicon family, labels in UI family) and the
        // parallel row-action list. Each row is one '\n'-separated line.
        let mut rows: Vec<Row> = Vec::new();
        // We must own the per-span strings; collect them, then build Attrs spans.
        let mut segs: Vec<(String, glyphon::Color, &'static str)> = Vec::new(); // (text, color, family)
        let chev = |open: bool| if open { theme::ICON_CHEVRON_DOWN } else { theme::ICON_CHEVRON_RIGHT };
        let mut newline = |segs: &mut Vec<(String, glyphon::Color, &'static str)>, rows: &Vec<Row>| {
            if !rows.is_empty() {
                segs.push(("\n".into(), dim, uifam));
            }
        };

        if self.session == Session::Stopped {
            // CALL STACK section.
            newline(&mut segs, &rows);
            segs.push((format!("{} ", chev(self.call_stack_open)), dim, icon));
            segs.push(("CALL STACK".into(), dim, uifam));
            rows.push(Row { act: RowAct::ToggleCallStack });
            if self.call_stack_open {
                for f in &self.frames {
                    newline(&mut segs, &rows);
                    let loc = f.path.as_deref().and_then(|p| std::path::Path::new(p).file_name()).map(|n| n.to_string_lossy().into_owned());
                    segs.push(("    ".into(), dim, uifam)); // indent (no chevron)
                    segs.push((f.name.clone(), fg, uifam));
                    if let Some(file) = loc {
                        segs.push((format!("   {file}:{}", f.line), dim, uifam));
                    }
                    rows.push(Row { act: RowAct::SelectFrame(f.id) });
                }
            }
            // VARIABLES section.
            newline(&mut segs, &rows);
            segs.push((format!("{} ", chev(self.variables_open)), dim, icon));
            segs.push(("VARIABLES".into(), dim, uifam));
            rows.push(Row { act: RowAct::ToggleVariables });
            if self.variables_open {
                // collect rows recursively
                let scopes_snapshot: Vec<(usize, ScopeView)> = self.scope_views();
                for (depth, v) in scopes_snapshot {
                    newline(&mut segs, &rows);
                    let ind = "  ".repeat(depth + 1);
                    if v.expandable {
                        segs.push((ind, dim, uifam));
                        segs.push((format!("{} ", chev(v.expanded)), dim, icon));
                    } else {
                        segs.push((format!("{ind}    "), dim, uifam));
                    }
                    segs.push((v.name, name_c, uifam));
                    if let Some(val) = v.value {
                        segs.push((" = ".into(), dim, uifam));
                        segs.push((val, fg, uifam));
                    }
                    rows.push(Row { act: RowAct::ToggleVar(v.var_ref) });
                }
            }
        }

        self.rows = rows;
        self.body.set_metrics(fs, Metrics::new(theme::UI_FONT_SIZE(), theme::UI_LINE_HEIGHT()));
        let spans: Vec<(&str, Attrs)> = segs
            .iter()
            .map(|(t, c, fam)| (t.as_str(), Attrs::new().family(Family::Name(fam)).color(*c)))
            .collect();
        let base = Attrs::new().family(Family::Name(uifam));
        self.body.set_rich_text(fs, spans, base, Shaping::Advanced);
        self.body.shape_until_scroll(fs, false);
    }

    /// Flatten scopes + their expanded variables into display rows.
    fn scope_views(&self) -> Vec<(usize, ScopeView)> {
        let mut out = Vec::new();
        for s in &self.scopes {
            out.push((0, ScopeView { name: s.name.clone(), value: None, var_ref: s.var_ref, expandable: true, expanded: s.expanded }));
            if s.expanded {
                Self::var_views(&s.vars, 1, &mut out);
            }
        }
        out
    }

    fn var_views(nodes: &[VarNode], depth: usize, out: &mut Vec<(usize, ScopeView)>) {
        for n in nodes {
            let val = if n.value.chars().count() > 120 { format!("{}…", n.value.chars().take(120).collect::<String>()) } else { n.value.clone() };
            out.push((depth, ScopeView { name: n.name.clone(), value: Some(val), var_ref: n.var_ref, expandable: n.var_ref > 0, expanded: n.expanded }));
            if n.expanded {
                Self::var_views(&n.children, depth + 1, out);
            }
        }
    }

    fn body_region(region: Rect) -> Rect {
        let t = theme::zpx(TOOLBAR_H);
        Rect { x: region.x, y: region.y + t, w: region.w, h: (region.h - t).max(0.0) }
    }

    fn toolbar_rects(region: Rect) -> [Rect; 7] {
        let bw = theme::zpx(BTN);
        let gap = theme::zpx(2.0);
        let y = region.y + (theme::zpx(TOOLBAR_H) - bw) * 0.5;
        let right = region.x + region.w - theme::zpx(8.0);
        std::array::from_fn(|i| {
            let x = right - (7 - i) as f32 * (bw + gap) + gap;
            Rect { x, y, w: bw, h: bw }
        })
    }

    pub fn update(&mut self, fs: &mut FontSystem, region: Rect) {
        self.last_w = region.w;
        let body = Self::body_region(region);
        let lh = theme::UI_LINE_HEIGHT();
        let content_h = self.rows.len() as f32 * lh + theme::zpx(TOP_PAD * 2.0);
        self.scroll.set_metrics(body, (body.w, content_h));
        let _ = fs;
    }

    /// Screen y of row `i` (top), accounting for scroll.
    fn row_y(&self, body: Rect, i: usize) -> f32 {
        let (_, sy) = self.scroll.offset();
        body.y + theme::zpx(TOP_PAD) + i as f32 * theme::UI_LINE_HEIGHT() - sy
    }

    fn row_at(&self, body: Rect, p: (f32, f32)) -> Option<usize> {
        if !body.contains(p) {
            return None;
        }
        let (_, sy) = self.scroll.offset();
        let lh = theme::UI_LINE_HEIGHT();
        let i = ((p.1 - (body.y + theme::zpx(TOP_PAD)) + sy) / lh).floor();
        if i < 0.0 {
            return None;
        }
        let i = i as usize;
        (i < self.rows.len()).then_some(i)
    }

    pub fn draw_quads(&self, region: Rect, now: std::time::Instant, bg: &mut Vec<Quad>, fg: &mut Vec<Quad>) {
        let t = theme::zpx(TOOLBAR_H);
        bg.push(Quad::new(region.x, region.y + t - 1.0, region.w, 1.0, theme::PANEL_BORDER()));
        // Hover highlight on the tree row under the cursor.
        let body = Self::body_region(region);
        if let Some(i) = self.hovered {
            if i < self.rows.len() {
                let y = self.row_y(body, i);
                if y + theme::UI_LINE_HEIGHT() > body.y && y < body.y + body.h {
                    bg.push(Quad::new(body.x, y, body.w, theme::UI_LINE_HEIGHT(), theme::TREE_HOVER()));
                }
            }
        }
        self.scroll.draw(now, fg);
    }

    pub fn draw_text<'a>(&'a self, region: Rect, areas: &mut Vec<TextArea<'a>>) {
        // Config label.
        let label_rect = Rect { x: region.x + theme::zpx(8.0), y: region.y, w: region.w - theme::zpx(8.0 + BTN * 7.0 + 16.0), h: theme::zpx(TOOLBAR_H) };
        self.config_label.push_in(label_rect.x, label_rect, label_rect, theme::FG_TEXT(), areas);
        // Toolbar glyphs.
        let rects = Self::toolbar_rects(region);
        let active = self.session != Session::Idle;
        let on = theme::FG_TEXT();
        let off = theme::FG_DIM();
        let g = |a: bool| if a { on } else { off };
        let colors = [on, g(active), g(active), g(active), g(active), g(active), g(!active)];
        for (i, b) in self.btns.iter().enumerate() {
            if i == 0 && self.session == Session::Running {
                self.btn_pause.draw(rects[0], on, areas);
            } else {
                b.draw(rects[i], colors[i], areas);
            }
        }
        // Body.
        let body = Self::body_region(region);
        if self.rows.is_empty() {
            self.hint.push_in(body.x + theme::zpx(PAD), Rect { x: body.x, y: body.y + theme::zpx(TOP_PAD), w: body.w, h: theme::UI_LINE_HEIGHT() }, body, theme::FG_DIM(), areas);
            return;
        }
        let (_, sy) = self.scroll.offset();
        areas.push(TextArea {
            buffer: &self.body,
            left: body.x + theme::zpx(PAD),
            top: body.y + theme::zpx(TOP_PAD) - sy,
            scale: 1.0,
            bounds: TextBounds { left: body.x as i32, top: body.y as i32, right: (body.x + body.w) as i32, bottom: (body.y + body.h) as i32 },
            default_color: theme::FG_TEXT(),
            custom_glyphs: &[],
        });
    }

    pub fn on_wheel(&mut self, p: (f32, f32), region: Rect, dy: f32) -> bool {
        if Self::body_region(region).contains(p) {
            return self.scroll.on_wheel(0.0, dy);
        }
        false
    }

    /// Track the hovered tree row (returns true if it changed → caller redraws).
    pub fn on_hover(&mut self, p: (f32, f32), region: Rect) -> bool {
        let was = self.hovered;
        self.hovered = self.row_at(Self::body_region(region), p);
        was != self.hovered
    }

    /// Is `p` over a clickable tree row (for pointer-cursor resolution)?
    pub fn over_row(&self, p: (f32, f32), region: Rect) -> bool {
        self.row_at(Self::body_region(region), p).is_some()
    }

    pub fn on_press(&mut self, pt: (f32, f32), region: Rect, fs: &mut FontSystem, out: &mut Vec<Intent>) -> bool {
        // Toolbar.
        let rects = Self::toolbar_rects(region);
        for (i, r) in rects.iter().enumerate() {
            if r.contains(pt) {
                let intent = match i {
                    0 => match self.session {
                        Session::Stopped => Intent::DebugContinue,
                        Session::Running => Intent::DebugPause,
                        Session::Idle => Intent::DebugStart { config_idx: self.selected },
                    },
                    1 => Intent::DebugStepOver,
                    2 => Intent::DebugStepIn,
                    3 => Intent::DebugStepOut,
                    4 => Intent::DebugStop,
                    5 => Intent::DebugStart { config_idx: self.selected },
                    _ => Intent::DebugAttachProcess,
                };
                out.push(intent);
                return true;
            }
        }
        // Config selector.
        let label_rect = Rect { x: region.x, y: region.y, w: region.w - theme::zpx(BTN * 7.0 + 16.0), h: theme::zpx(TOOLBAR_H) };
        if label_rect.contains(pt) && !self.configs.is_empty() {
            self.selected = (self.selected + 1) % self.configs.len();
            out.push(Intent::DebugSelectConfig(self.selected));
            return true;
        }
        // Tree row.
        let body = Self::body_region(region);
        if let Some(i) = self.row_at(body, pt) {
            let act = self.rows[i].act.clone();
            match act {
                RowAct::ToggleCallStack => { self.call_stack_open = !self.call_stack_open; self.rebuild(fs); }
                RowAct::ToggleVariables => { self.variables_open = !self.variables_open; self.rebuild(fs); }
                RowAct::SelectFrame(id) => out.push(Intent::DebugSelectFrame(id)),
                RowAct::ToggleVar(var_ref) => {
                    if let Some(need) = self.toggle_var(var_ref) {
                        out.push(Intent::DebugExpandVar(need));
                    }
                    self.rebuild(fs);
                }
                RowAct::None => {}
            }
            return true;
        }
        body.contains(pt)
    }
}

/// A flattened scope/variable display row.
struct ScopeView {
    name: String,
    value: Option<String>,
    var_ref: i64,
    expandable: bool,
    expanded: bool,
}
