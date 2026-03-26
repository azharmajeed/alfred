# Phase 2 — Multiplexing

**Date:** 2026-03-26
**Last updated:** 2026-03-26
**Result:** `cargo run --bin alfred-app` opens a terminal with full split-pane multiplexing, workspace tab switching, mouse-wheel scroll, and correct HiDPI rendering.

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
| Mouse-wheel scroll — routes to `TerminalState::scroll_display` | ✅ |
| HiDPI double-scale bug fixed — text renders at correct size | ✅ |
| `needs_reshape` flag — skips font-shaping for unchanged panes | ✅ |
| `collect_frame()` — cells + cursor in a single terminal lock | ✅ |
| 15 unit tests for `PaneTree` layout and mutation | ✅ |

---

## New Files

```
crates/alfred-app/src/
└── workspace/
    ├── mod.rs
    ├── pane.rs       — Pane struct (id, terminal, pty_tx, dirty)
    ├── layout.rs     — PaneTree, PhysRect, SplitDir, layout/split/remove_leaf + tests
    └── manager.rs    — WorkspaceManager, Workspace
```

**Updated:**
- `terminal/pty.rs` — `run_pty(pane_id, …)` tags `UserEvent::PtyOutput/PtyExited` with `pane_id`
- `terminal/emulator.rs` — `collect_frame()` single-lock helper; `scroll_display(delta)`
- `app.rs` — uses `WorkspaceManager`; tracks `ModifiersState`; multiplexer shortcuts; `MouseWheel` handler; atomic `swap` dirty flag
- `renderer.rs` — `render(&[PaneView])` with per-pane `Buffer` map; `needs_reshape` optimisation; `scale: 1.0` HiDPI fix
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
| Mouse wheel up/down | Scroll terminal history |

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

`split(target, dir, new_id)` finds the leaf with `target` and replaces it with a `Split(old_leaf, new_leaf)` node using `std::mem::replace` to take ownership without borrow-checker conflicts.

`remove_leaf(target)` replaces `Split(target, sibling)` with just the sibling by taking ownership of the entire tree node via `std::mem::replace`. An early bug where non-target leaves had their IDs clobbered during recursion was caught by the unit test suite.

### Per-pane Buffer map

The renderer keeps a `HashMap<PaneId, Buffer>` so glyphon's glyph atlas is warm across frames. Each frame:
1. **Pass 1** (mutable): for each pane, resize the buffer to fit its rect. If `needs_reshape` is set, call `set_rich_text` + `shape_until_scroll`; otherwise skip (the buffer already has the correct content).
2. **Prune** stale buffers for panes no longer in the layout.
3. **Pass 2** (immutable): build `Vec<TextArea<'_>>` and pass to `glyphon::TextRenderer::prepare`.

Separating the two passes avoids `&mut self.font_system` / `&self.pane_buffers[id]` borrow conflicts.

### HiDPI double-scale fix

The original Phase 1 code set `TextArea.scale = scale_factor` while also passing physical-unit metrics (`Metrics::new(FONT_SIZE * scale_factor, …)`). glyphon applies `TextArea.scale` to the glyph positions computed from those metrics, so both were multiplied together — double-scaling at anything above 100% DPI.

Fix: `TextArea.scale = 1.0`. The buffer metrics are already physical; no further scaling should be applied.

### Scale-aware cell sizing in WorkspaceManager

`WorkspaceManager` stores `scale_factor: f32`. All cell-dimension calculations use:
```
physical_cell_w = 9.0 (logical) × scale_factor
physical_cell_h = 18.0 (logical) × scale_factor
cols = rect.w / physical_cell_w
rows = rect.h / physical_cell_h
```

This fixes a secondary bug where `resize_all` and `split_active_pane` were dividing physical pixel rects by the logical constant `9.0`, giving too many cols/rows on HiDPI screens and corrupting the PTY window size.

### Dirty-flag optimisation

`PtyOutput` sets `pane.dirty = true` and requests a redraw. In `RedrawRequested`:
- `pane.dirty.swap(false, AcqRel)` atomically reads and clears the flag.
- If it was set: `terminal.lock()` → `collect_frame()` (cells + cursor in one pass) → `needs_reshape: true` in `PaneView`.
- If not set: pass empty cells with `needs_reshape: false` — the renderer skips reshaping and reuses the existing `Buffer`.

Focus-switch operations mark all panes dirty so the cursor highlight updates correctly on both the old and new active pane.

### Mouse-wheel scroll

```rust
WindowEvent::MouseWheel { delta, .. } => {
    let lines = match delta {
        LineDelta(_, y)  => y as i32,
        PixelDelta(pos)  => (pos.y / 20.0) as i32,
    };
    term.scroll_display(lines);  // alacritty_terminal::grid::Scroll::Delta
}
```

Positive delta scrolls up through history; negative scrolls back to the bottom.

### Pane rect in physical pixels

`PhysRect { x, y, w, h }` is always in physical (device) pixels.
- `layout()` returns physical rects derived from `window.inner_size()` (physical).
- Cell cols/rows are computed as `rect.w / (CELL_W_LOGICAL × scale_factor)`.
- `TextArea.left/top` are physical coordinates passed directly to glyphon.

### Modifier tracking

`WindowEvent::ModifiersChanged(m)` updates `AppInner::modifiers: ModifiersState`.
Shortcuts use `physical_key: PhysicalKey::Code(KeyCode::…)` so they work regardless of keyboard layout (US `]` and `[` remain `BracketRight`/`BracketLeft` on all layouts).

---

## Unit Tests

`workspace/layout.rs` contains 15 tests in `#[cfg(test)]`:

| Test | What it covers |
|---|---|
| `single_leaf_fills_rect` | A single pane gets the full window rect |
| `vertical_split_sums_to_full_width` | Left + divider + right = total width |
| `horizontal_split_sums_to_full_height` | Top + divider + bottom = total height |
| `vertical_split_default_ratio_is_even` | 50/50 split at ratio=0.5 |
| `non_zero_origin_preserved` | Sub-rects respect non-zero x/y origins |
| `leaves_single` | Single leaf returns `[id]` |
| `leaves_after_two_splits` | All three pane IDs present; correct order |
| `split_returns_false_for_missing_id` | Split on unknown ID is a no-op |
| `split_returns_true_for_existing_id` | Split on known ID succeeds |
| `split_twice_yields_three_leaves` | Three-pane tree after two splits |
| `remove_left_child_leaves_right` | Removing left leaf collapses to right |
| `remove_right_child_leaves_left` | Removing right leaf collapses to left |
| `remove_middle_child_in_three_pane_tree` | Deep removal keeps correct siblings |
| `layout_order_left_before_right` | Vertical split: left pane x < right pane x |
| `layout_order_top_before_bottom` | Horizontal split: top pane y < bottom pane y |

The `remove_middle_child` test caught a real bug: non-target `Leaf` nodes were having their IDs replaced with `target_id` during recursive removal.

---

## Known Limitations (to fix in later phases)

- **No split divider border** — the 2px gap between panes shows as the dark background; a visible accent-colour line is planned for Phase 3.
- **Fixed split ratio** — ratio is always 0.5; no drag-to-resize yet.
- **No tab bar UI** — workspace tabs exist in state but are not yet rendered in the window chrome (Phase 3).
- **No cell background colours** — `TermCell.bg` stored but not rendered (Phase 3).
- **No bold/italic** — `Cell.Flags` not yet mapped to glyphon `Attrs`.
- **HiDPI cell sizing** — cell dims still hardcoded (9×18 logical px); should derive from actual font metrics at runtime.
