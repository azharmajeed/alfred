# Phase 2 — Multiplexing

**Date:** 2026-03-26
**Result:** `cargo run --bin alfred-app` opens a terminal with full split-pane multiplexing and workspace tab switching.

---

## What Was Built

| Feature | Status |
|---|---|
| Binary split tree (`PaneTree`) — vertical and horizontal splits | ✅ |
| `Workspace` — owns a pane tree + active pane focus | ✅ |
| `WorkspaceManager` — multiple workspaces, split/focus/resize/remove | ✅ |
| Per-pane PTY — each pane spawns its own independent shell | ✅ |
| `UserEvent` tagged with `pane_id` — per-pane event routing | ✅ |
| Multi-pane renderer — one `glyphon::Buffer` per pane, single wgpu pass | ✅ |
| Active pane highlighted (bright cursor), inactive dimmed | ✅ |
| Pane resize on window resize — recomputes cols/rows per pane rect | ✅ |
| Workspace tabs — Ctrl+Shift+T creates, Ctrl+Tab / Ctrl+Shift+Tab switches | ✅ |
| PTY exit removes pane; last pane exit closes the app | ✅ |

---

## New Files

```
crates/alfred-app/src/
└── workspace/
    ├── mod.rs
    ├── pane.rs       — Pane struct (id, terminal, pty_tx, dirty)
    ├── layout.rs     — PaneTree, PhysRect, SplitDir, layout/split/remove_leaf
    └── manager.rs    — WorkspaceManager, Workspace
```

**Updated:**
- `terminal/pty.rs` — `run_pty(pane_id, …)` tags `UserEvent::PtyOutput/PtyExited` with `pane_id`
- `app.rs` — uses `WorkspaceManager`; tracks `ModifiersState`; handles split/focus shortcuts
- `renderer.rs` — `render(&[PaneView])` with per-pane `Buffer` map and `TextArea` slice
- `main.rs` — added `mod workspace`

---

## Keyboard Shortcuts

| Shortcut | Action |
|---|---|
| `Ctrl+Shift+E` | Split active pane vertically (left \| right) |
| `Ctrl+Shift+O` | Split active pane horizontally (top / bottom) |
| `Ctrl+Shift+]` | Focus next pane |
| `Ctrl+Shift+[` | Focus previous pane |
| `Ctrl+Shift+W` | Close active pane |
| `Ctrl+Shift+T` | New workspace tab |
| `Ctrl+Tab` | Next workspace |
| `Ctrl+Shift+Tab` | Previous workspace |

---

## Key Design Decisions

### PaneTree — binary split tree

```rust
pub enum PaneTree {
    Leaf(PaneId),
    Split {
        dir: SplitDir,   // Vertical | Horizontal
        ratio: f32,      // 0..1 where the divider sits (default 0.5)
        left: Box<PaneTree>,
        right: Box<PaneTree>,
    },
}
```

`layout(rect: PhysRect) → Vec<(PaneId, PhysRect)>` walks the tree top-down,
dividing the rect at each split and returning a flat list of (pane_id, assigned_rect) for the renderer.

`split(target, dir, new_id)` finds the leaf with `target` and replaces it with a `Split(old_leaf, new_leaf)` node.

`remove_leaf(target)` handles removal by replacing a `Split(target, sibling)` with just the sibling, using `std::mem::replace` to avoid borrow conflicts.

### Per-pane Buffer map

The renderer keeps a `HashMap<PaneId, Buffer>` so glyphon's glyph atlas is warm across frames. Each frame:
1. **Pass 1** (mutable): update/create each pane's Buffer with new cell content.
2. **Prune** stale buffers for panes no longer in the layout.
3. **Pass 2** (immutable): build `Vec<TextArea<'_>>` and pass to `glyphon::TextRenderer::prepare`.

Separating the two passes avoids `&mut self.font_system` / `&self.pane_buffers[id]` borrow conflicts.

### Pane rect in physical pixels

`PhysRect { x, y, w, h }` is always in physical (device) pixels.
- `layout()` returns physical rects derived from `window.inner_size()` (physical).
- Cell cols/rows are computed as `rect.w / CELL_W` where `CELL_W = 9.0` physical pixels.
- `TextArea.left/top` are physical coordinates; glyphon scales them by `TextArea.scale`.

This is consistent with Phase 1's HiDPI approach where cell dims are in physical pixels.

### Modifier tracking

`WindowEvent::ModifiersChanged(m)` updates `AppInner::modifiers: ModifiersState`.
Shortcuts use `physical_key: PhysicalKey::Code(KeyCode::…)` so they work regardless of keyboard layout (layout-independent physical key codes, not logical characters).

---

## Known Limitations (to fix in later phases)

- **No split divider border** — the 2px gap between panes shows as the dark background; a visible accent-colour line is planned.
- **Fixed split ratio** — ratio is always 0.5; no drag-to-resize yet.
- **No tab bar UI** — workspace tabs exist in state but are not yet rendered in the window chrome.
- **No scrollback UI** — wheel scroll not yet wired.
- **No cell background colours** — planned for Phase 3.
- **No bold/italic** — `Cell.Flags` not yet mapped to glyphon `Attrs`.
- **HiDPI cell sizing** — cell dims still hardcoded (9×18 physical px); should derive from font metrics.
