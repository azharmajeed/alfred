use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use tokio::runtime::Runtime;
use winit::event_loop::EventLoopProxy;

use super::layout::{PhysRect, PaneTree, SplitDir};
use super::pane::{Pane, PaneId};
use crate::app::UserEvent;

/// Logical (unscaled) cell dimensions in pixels — same as renderer constants.
const CELL_W_LOGICAL: f32 = 9.0;
const CELL_H_LOGICAL: f32 = 18.0;

// ── Workspace ─────────────────────────────────────────────────────────────────

pub struct Workspace {
    pub name: String,
    pub tree: PaneTree,
    pub active_pane: PaneId,
    pub panes: HashMap<PaneId, Pane>,
}

// ── WorkspaceManager ─────────────────────────────────────────────────────────

pub struct WorkspaceManager {
    pub workspaces: Vec<Workspace>,
    pub active: usize,
    next_id: PaneId,
    rt: Arc<Runtime>,
    /// HiDPI scale factor — converts logical cell dims to physical pixels.
    pub scale_factor: f32,
}

impl WorkspaceManager {
    /// Create a manager with one initial workspace containing one pane.
    pub fn new(
        rt: Arc<Runtime>,
        cols: u16,
        rows: u16,
        proxy: EventLoopProxy<UserEvent>,
        scale_factor: f32,
    ) -> Self {
        let mut mgr = Self {
            workspaces: Vec::new(),
            active: 0,
            next_id: 0,
            rt,
            scale_factor,
        };
        mgr.new_workspace("Workspace 1", cols, rows, proxy);
        mgr
    }

    // ── Workspace operations ────────────────────────────────────────────────

    /// Add a new workspace with a single shell pane.
    pub fn new_workspace(
        &mut self,
        name: &str,
        cols: u16,
        rows: u16,
        proxy: EventLoopProxy<UserEvent>,
    ) {
        let id = self.alloc_id();
        let (pty_tx, pty_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        let pane = Pane::new(id, cols, rows, pty_tx);
        let terminal = pane.terminal.clone();
        let dirty = pane.dirty.clone();

        self.rt.spawn(async move {
            crate::terminal::pty::run_pty(id, cols, rows, pty_rx, terminal, dirty, proxy).await;
        });

        let mut panes = HashMap::new();
        panes.insert(id, pane);

        self.workspaces.push(Workspace {
            name: name.to_string(),
            tree: PaneTree::new_leaf(id),
            active_pane: id,
            panes,
        });
    }

    pub fn next_workspace(&mut self) {
        if !self.workspaces.is_empty() {
            self.active = (self.active + 1) % self.workspaces.len();
        }
    }

    pub fn prev_workspace(&mut self) {
        if !self.workspaces.is_empty() {
            let n = self.workspaces.len();
            self.active = (self.active + n - 1) % n;
        }
    }

    // ── Pane operations ─────────────────────────────────────────────────────

    /// Split the currently focused pane, spawning a new shell in the new half.
    pub fn split_active_pane(
        &mut self,
        dir: SplitDir,
        window_rect: PhysRect,
        proxy: EventLoopProxy<UserEvent>,
    ) {
        let new_id = self.alloc_id();

        // 1. Compute the new pane's initial size from half the active pane's rect.
        let phys_cw = CELL_W_LOGICAL * self.scale_factor;
        let phys_ch = CELL_H_LOGICAL * self.scale_factor;
        let (active_id, cols, rows) = {
            let ws = &self.workspaces[self.active];
            let layout = ws.tree.layout(window_rect);
            let active_rect = layout
                .iter()
                .find(|(id, _)| *id == ws.active_pane)
                .map(|(_, r)| *r)
                .unwrap_or(window_rect);
            let (cols, rows) = pane_cols_rows(active_rect, dir, phys_cw, phys_ch);
            (ws.active_pane, cols, rows)
        };

        // 2. Spawn PTY for the new pane (no ws borrow held).
        let (pty_tx, pty_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        let pane = Pane::new(new_id, cols, rows, pty_tx);
        let terminal = pane.terminal.clone();
        let dirty = pane.dirty.clone();

        self.rt.spawn(async move {
            crate::terminal::pty::run_pty(new_id, cols, rows, pty_rx, terminal, dirty, proxy)
                .await;
        });

        // 3. Update the tree and pane map.
        let ws = &mut self.workspaces[self.active];
        ws.tree.split(active_id, dir, new_id);
        ws.panes.insert(new_id, pane);
        ws.active_pane = new_id;
    }

    /// Focus the next pane (wraps around).
    pub fn focus_next_pane(&mut self) {
        let ws = &mut self.workspaces[self.active];
        let leaves = ws.tree.leaves();
        if leaves.len() < 2 {
            return;
        }
        let pos = leaves.iter().position(|&id| id == ws.active_pane).unwrap_or(0);
        ws.active_pane = leaves[(pos + 1) % leaves.len()];
        // Mark all panes dirty so the cursor highlight updates on both old and new focus.
        for pane in ws.panes.values() {
            pane.dirty.store(true, Ordering::Release);
        }
    }

    /// Focus the previous pane (wraps around).
    pub fn focus_prev_pane(&mut self) {
        let ws = &mut self.workspaces[self.active];
        let leaves = ws.tree.leaves();
        if leaves.len() < 2 {
            return;
        }
        let n = leaves.len();
        let pos = leaves.iter().position(|&id| id == ws.active_pane).unwrap_or(0);
        ws.active_pane = leaves[(pos + n - 1) % n];
        for pane in ws.panes.values() {
            pane.dirty.store(true, Ordering::Release);
        }
    }

    /// Remove a pane (e.g. when its PTY exits or the user closes it).
    /// Returns `true` if the last pane was removed (app should exit).
    pub fn remove_pane(&mut self, pane_id: PaneId) -> bool {
        let ws = &self.workspaces[self.active];
        let leaves = ws.tree.leaves();

        if !leaves.contains(&pane_id) {
            return false;
        }

        if leaves.len() == 1 {
            // Last pane — signal close.
            return true;
        }

        // Pick the next pane to focus after removal.
        let pos = leaves.iter().position(|&id| id == pane_id).unwrap_or(0);
        let next_active = if pos + 1 < leaves.len() {
            leaves[pos + 1]
        } else {
            leaves[pos - 1]
        };

        let ws = &mut self.workspaces[self.active];
        ws.panes.remove(&pane_id);
        ws.tree.remove_leaf(pane_id);
        ws.active_pane = next_active;

        false
    }

    // ── Resize ──────────────────────────────────────────────────────────────

    /// After the window resizes, recompute cols/rows for every visible pane.
    pub fn resize_all(&mut self, window_rect: PhysRect) {
        let (layout, phys_cw, phys_ch) = {
            let ws = &self.workspaces[self.active];
            let cw = CELL_W_LOGICAL * self.scale_factor;
            let ch = CELL_H_LOGICAL * self.scale_factor;
            (ws.tree.layout(window_rect), cw, ch)
        };

        let ws = &mut self.workspaces[self.active];
        for (pane_id, rect) in layout {
            if let Some(pane) = ws.panes.get_mut(&pane_id) {
                let cols = (rect.w as f32 / phys_cw).floor().max(1.0) as u16;
                let rows = (rect.h as f32 / phys_ch).floor().max(1.0) as u16;
                if let Ok(mut term) = pane.terminal.lock() {
                    term.resize(cols, rows);
                }
                pane.dirty.store(true, Ordering::Release);
            }
        }
    }

    // ── Dirty tracking ──────────────────────────────────────────────────────

    /// `true` if any pane in the active workspace has pending output to render.
    pub fn any_dirty(&self) -> bool {
        self.workspaces[self.active]
            .panes
            .values()
            .any(|p| p.dirty.load(Ordering::Acquire))
    }

    // ── Accessors ───────────────────────────────────────────────────────────

    pub fn active_workspace(&self) -> &Workspace {
        &self.workspaces[self.active]
    }

    pub fn active_workspace_mut(&mut self) -> &mut Workspace {
        &mut self.workspaces[self.active]
    }

    // ── Internal helpers ────────────────────────────────────────────────────

    fn alloc_id(&mut self) -> PaneId {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        id
    }
}

/// Compute the (cols, rows) for a new pane that will occupy half of `rect`
/// after a split in direction `dir`. `phys_cw/ch` are physical cell dimensions.
fn pane_cols_rows(rect: PhysRect, dir: SplitDir, phys_cw: f32, phys_ch: f32) -> (u16, u16) {
    match dir {
        SplitDir::Vertical => {
            let half_w = rect.w / 2;
            let cols = (half_w as f32 / phys_cw).floor().max(1.0) as u16;
            let rows = (rect.h as f32 / phys_ch).floor().max(1.0) as u16;
            (cols, rows)
        }
        SplitDir::Horizontal => {
            let half_h = rect.h / 2;
            let cols = (rect.w as f32 / phys_cw).floor().max(1.0) as u16;
            let rows = (half_h as f32 / phys_ch).floor().max(1.0) as u16;
            (cols, rows)
        }
    }
}
