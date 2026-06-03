// Command-palette command set + the palette/find-bar UI state.

#[derive(Clone, Copy)]
pub enum Command {
    Save,
    Close,
    Find,
    Undo,
    Redo,
    SelectAll,
    ToggleSidebar,
    NewFile,
    OpenSettings,
    OpenDefaultSettings,
    ToggleTerminal,
    OpenFolder,
    ColorTheme,
}   

pub const COMMANDS: &[(Command, &str, &str)] = &[
    (Command::Save, "File: Save", "Ctrl+S"),
    (Command::NewFile, "File: New Untitled", ""),
    (Command::OpenFolder, "File: Open Folder", "Ctrl+O"),
    (Command::Close, "File: Close Tab", "Ctrl+W"),
    (Command::Find, "Edit: Find", "Ctrl+F"),
    (Command::Undo, "Edit: Undo", "Ctrl+Z"),
    (Command::Redo, "Edit: Redo", "Ctrl+Y"),
    (Command::SelectAll, "Edit: Select All", "Ctrl+A"),
    (Command::ToggleSidebar, "View: Toggle Sidebar", ""),
    (Command::OpenSettings, "Preferences: Open Settings (JSON)", ""),
    (Command::OpenDefaultSettings, "Preferences: Open Default Settings (JSON)", ""),
    (Command::ColorTheme, "Preferences: Color Theme", ""),
    (Command::ToggleTerminal, "View: Toggle Terminal", "Ctrl+`"),
];

/// What a quick-pick selection does. Each variant carries no data here; the chosen
/// item's `label` is read from `PaletteState` when the pick is committed.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PickKind {
    SetColorTheme,
}

/// One row in a quick-pick list (dynamic, unlike the fixed `COMMANDS`).
#[derive(Clone)]
pub struct PickItem {
    pub label: String,        // committed value / primary text
    pub detail: String,       // dim right-hand hint
    pub line: Option<usize>,  // 1-based target line (go-to-symbol)
}

impl PickItem {
    pub fn new(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self { label: label.into(), detail: detail.into(), line: None }
    }
    pub fn at_line(label: impl Into<String>, detail: impl Into<String>, line: usize) -> Self {
        Self { label: label.into(), detail: detail.into(), line: Some(line) }
    }
}

/// Quick-open modes, VSCode-style. Most are driven by the input's leading prefix
/// (`>` commands, `@` symbols, `:` line, none = files); `QuickPick` is a one-off
/// chooser (e.g. themes) opened programmatically.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PaletteMode {
    Commands,         // `>` — run a command
    Files,            // (no prefix) — go to file
    Symbols,          // `@` — go to symbol in the active file
    GoToLine,         // `:` — go to line
    QuickPick(PickKind),
}

pub struct PaletteState {
    pub active: bool,
    pub selected: usize,
    pub filtered: Vec<usize>,
    pub mode: PaletteMode,
    pub items: Vec<PickItem>, // the quick-pick source (empty in Commands mode)
    pub scroll: f32,          // list scroll offset in px (clamped/followed in render)
    pub follow_selection: bool, // scroll to keep the selection visible next frame
}

impl PaletteState {
    pub fn new() -> Self {
        let filtered: Vec<usize> = (0..COMMANDS.len()).collect();
        Self {
            active: false,
            selected: 0,
            filtered,
            mode: PaletteMode::Commands,
            items: Vec::new(),
            scroll: 0.0,
            follow_selection: true,
        }
    }
    /// Number of rows in the active source (commands or item list).
    fn source_len(&self) -> usize {
        match self.mode {
            PaletteMode::Commands => COMMANDS.len(),
            _ => self.items.len(),
        }
    }
    /// The display text of row `i` in the active source (for filtering).
    fn row_text(&self, i: usize) -> String {
        match self.mode {
            PaletteMode::Commands => COMMANDS[i].1.to_lowercase(),
            _ => self.items[i].label.to_lowercase(),
        }
    }
    pub fn refilter(&mut self, query: &str) {
        let q = query.to_lowercase();
        self.filtered = (0..self.source_len())
            .filter(|&i| q.is_empty() || self.row_text(i).contains(&q))
            .take(500) // cap the rendered set (go-to-file can have thousands of matches)
            .collect();
        if self.selected >= self.filtered.len() {
            self.selected = self.filtered.len().saturating_sub(1);
        }
        self.scroll = 0.0; // new results → back to the top
        self.follow_selection = true;
    }
    pub fn open(&mut self) {
        self.mode = PaletteMode::Commands;
        self.items.clear();
        self.active = true;
        self.selected = 0;
        self.refilter("");
    }
    /// Open as a dynamic quick-pick over `items`, committing via `kind`.
    pub fn open_quick_pick(&mut self, kind: PickKind, items: Vec<PickItem>) {
        self.mode = PaletteMode::QuickPick(kind);
        self.items = items;
        self.active = true;
        self.selected = 0;
        self.refilter("");
    }
    /// Switch the source for a prefix-driven mode (Files/Symbols), keeping the
    /// palette open. Resets the selection to the top.
    pub fn set_source(&mut self, mode: PaletteMode, items: Vec<PickItem>) {
        self.mode = mode;
        self.items = items;
        self.selected = 0;
        self.refilter("");
    }
    /// The selected item in any item-based mode (Files/Symbols/QuickPick).
    pub fn selected_item(&self) -> Option<&PickItem> {
        self.filtered.get(self.selected).and_then(|&i| self.items.get(i))
    }
    /// The selected quick-pick `(kind, label)`, if in quick-pick mode.
    pub fn selected_pick(&self) -> Option<(PickKind, String)> {
        match self.mode {
            PaletteMode::QuickPick(kind) => self
                .filtered
                .get(self.selected)
                .and_then(|&i| self.items.get(i))
                .map(|it| (kind, it.label.clone())),
            _ => None,
        }
    }
    pub fn close(&mut self) {
        self.active = false;
    }
    /// Move the selection down one row (wraps to the top).
    pub fn select_next(&mut self) {
        if !self.filtered.is_empty() {
            self.selected = (self.selected + 1) % self.filtered.len();
            self.follow_selection = true;
        }
    }
    /// Move the selection up one row (wraps to the bottom).
    pub fn select_prev(&mut self) {
        if !self.filtered.is_empty() {
            self.selected = if self.selected == 0 {
                self.filtered.len() - 1
            } else {
                self.selected - 1
            };
            self.follow_selection = true;
        }
    }
    /// The command under the current selection, if any (Commands mode only).
    pub fn selected_command(&self) -> Option<Command> {
        if self.mode != PaletteMode::Commands {
            return None;
        }
        self.filtered.get(self.selected).map(|&i| COMMANDS[i].0)
    }
}

pub struct FindBarState {
    pub active: bool,                       // the widget is open/visible
    pub focused: bool,                      // a find/replace input has keyboard focus
    pub on_replace: bool,                   // focus is on the replace input (vs find)
    pub replace_open: bool,                 // the replace row is expanded
    pub opts: crate::search::SearchOpts,    // case / whole-word / regex
    pub matches: Vec<(usize, usize)>,       // byte ranges of matches in the active doc
    pub index: Option<usize>,               // current match within `matches`
}

impl FindBarState {
    pub fn new() -> Self {
        Self {
            active: false,
            focused: false,
            on_replace: false,
            replace_open: false,
            opts: crate::search::SearchOpts::default(),
            matches: Vec::new(),
            index: None,
        }
    }
}
