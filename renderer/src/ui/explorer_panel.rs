// Explorer (file-tree) panel logic. The tree data itself lives in the shared
// `Workspace` (the editor's file-open flow reads it too); this panel owns the
// inline create/rename field state and the logic that drives it.
//
// NOTE (refactor staging): tree-click routing still lives on `App` (the right-
// click menu now goes through the generic ctx-menu system there); folding it in
// is a follow-up. The inline-create field buffer (`gpu.create_input`) stays in
// `gpu` since it's a glyph buffer.

use crate::gpu::GpuState;
use crate::widgets::{ScrollOpts, ScrollView};
use crate::workspace::Workspace;
use crate::PendingCreate;

pub struct ExplorerPanel {
    /// Inline new-file / new-folder / rename field, when active.
    pub creating: Option<PendingCreate>,
    /// Vertical scroll for the file tree (content can exceed the viewport).
    pub scroll: ScrollView,
}

impl ExplorerPanel {
    pub fn new() -> Self {
        Self {
            creating: None,
            scroll: ScrollView::new(ScrollOpts::vertical()),
        }
    }

    /// Start an inline create relative to the current tree selection: inside the
    /// selected folder, or beside the selected file (root when nothing is selected).
    pub fn begin_create(&mut self, is_dir: bool, selected: Option<usize>, ws: &mut Workspace, gpu: &mut GpuState) {
        let nodes = &ws.tree.nodes;
        let (parent, row, depth) = match selected.and_then(|i| nodes.get(i).map(|n| (i, n))) {
            Some((i, n)) if n.is_dir => {
                let path = n.path.clone();
                let depth = n.depth + 1;
                ws.tree.expand(&path);
                (path, i + 1, depth)
            }
            Some((i, n)) => {
                let parent = n
                    .path
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| ws.tree.root.clone());
                (parent, i, n.depth)
            }
            None => (ws.tree.root.clone(), 0, 0),
        };
        self.creating = Some(PendingCreate { is_dir, parent, row, depth, rename_from: None });
        gpu.create_input.clear(&mut gpu.font_system);
        gpu.create_input
            .set_placeholder(&mut gpu.font_system, if is_dir { " folder name" } else { " file name" });
        gpu.create_input.focus(true);
    }

    /// Begin an inline rename of a tree node: the field replaces the node's row,
    /// pre-filled with its current name.
    pub fn begin_rename(&mut self, idx: usize, ws: &Workspace, gpu: &mut GpuState) {
        let Some(n) = ws.tree.nodes.get(idx) else {
            return;
        };
        let parent = n
            .path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| ws.tree.root.clone());
        let name = n.name.clone();
        self.creating = Some(PendingCreate {
            is_dir: n.is_dir,
            parent,
            row: idx,
            depth: n.depth,
            rename_from: Some(n.path.clone()),
        });
        gpu.create_input.set_text(&mut gpu.font_system, &name);
        gpu.create_input.focus(true);
    }

    /// Finish an inline create/rename: apply if a non-empty name was typed (opening
    /// new files), otherwise just dismiss the field.
    pub fn commit_create(&mut self, ws: &mut Workspace, gpu: &mut GpuState) {
        let Some(pc) = self.creating.take() else {
            return;
        };
        let name = gpu.create_input.text().trim().to_string();
        gpu.create_input.focus(false);
        if name.is_empty() {
            return;
        }
        if let Some(from) = pc.rename_from {
            let to = pc.parent.join(&name);
            if to != from && std::fs::rename(&from, &to).is_ok() {
                ws.tree.refresh();
                // Re-point any open document at the renamed path.
                for d in ws.documents.iter_mut() {
                    if d.path.as_deref() == Some(from.as_path()) {
                        d.path = Some(to.clone());
                        d.name = name.clone();
                    }
                }
            }
        } else if let Ok(path) = ws.create_entry(&pc.parent, &name, pc.is_dir) {
            if !pc.is_dir {
                let _ = ws.open_file(&path, &mut gpu.font_system);
            }
        }
    }

    pub fn cancel_create(&mut self, gpu: &mut GpuState) {
        self.creating = None;
        gpu.create_input.focus(false);
    }
}
