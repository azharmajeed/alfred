# Phase 1 — Core Terminal Scaffold

**Date:** 2026-03-23
**Last updated:** 2026-03-26
**Result:** `cargo run --bin alfred-app` opens a GPU-accelerated terminal window running PowerShell with full text rendering, keyboard input, and a visible cursor.

---

## What Was Built

A fully functional single-pane terminal window:

| Feature | Status |
|---|---|
| Cargo workspace (`alfred-app` + `alfred-cli`) | ✅ |
| winit window (1200×800 default) | ✅ |
| wgpu 23 surface — DX12 on Windows, Vulkan on Linux | ✅ |
| PTY spawn (portable-pty 0.9) — pwsh/powershell/cmd on Windows | ✅ |
| alacritty_terminal 0.24 VT grid | ✅ |
| glyphon 0.7 GPU text rendering | ✅ |
| Keyboard input → PTY (full key table) | ✅ |
| Window resize → PTY + terminal grid resize | ✅ |
| Cursor position tracking + visible block cursor | ✅ |
| 256-colour + truecolor support | ✅ |
| HiDPI / DPI scaling | ✅ |
| VT response forwarding (DSR / PtyWrite) | ✅ |
| `alfred` CLI stub (Phase 5 IPC not yet wired) | ✅ |

---

## Files Created

```
alfred/
├── Cargo.toml                              # workspace, resolver = "2"
├── .gitignore
├── crates/
│   ├── alfred-app/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs                     # EventLoop::<UserEvent> entry point
│   │       ├── app.rs                      # ApplicationHandler, key routing
│   │       ├── renderer.rs                 # wgpu + glyphon text renderer
│   │       ├── terminal/
│   │       │   ├── mod.rs
│   │       │   ├── emulator.rs             # alacritty_terminal wrapper
│   │       │   └── pty.rs                  # portable-pty spawn + async I/O
│   │       └── platform/
│   │           ├── mod.rs                  # gpu_backends(), default_shell()
│   │           ├── windows.rs              # pwsh > powershell > cmd detection
│   │           └── linux.rs               # $SHELL detection
│   └── alfred-cli/
│       ├── Cargo.toml
│       └── src/main.rs                     # clap CLI stub
└── docs/
    └── phase1-scaffold.md                  # this file
```

---

## Key Design Decisions

### winit 0.30 — ApplicationHandler pattern
winit 0.30 uses the `ApplicationHandler<UserEvent>` trait instead of the old closure-based `run`. The `resumed` callback creates the window (important on Android; on desktop it fires once). Custom events (`UserEvent::PtyOutput`) are sent from tokio tasks via `EventLoopProxy`.

### Threading model
```
Main thread (winit event loop)
  │  processes UserEvent::PtyOutput → updates terminal → request_redraw
  │  handles keyboard → sends bytes to pty_writer channel
  │
Tokio runtime (2 worker threads)
  ├── PTY reader task  (spawn_blocking)
  │     reads PTY → EventLoopProxy::send_event(PtyOutput)
  └── PTY writer task
        receives from mpsc channel → writes to PTY master
        channel is shared by:
          - keyboard input (app.rs KeyboardInput handler)
          - VT PtyWrite responses (EventProxy::send_event)
```

### VT PtyWrite response forwarding (critical for PowerShell / cmd.exe)
`alacritty_terminal` emits `Event::PtyWrite(s)` for terminal responses that must be sent back to the child process. The most common case is replying to a **DSR cursor-position query** (`\x1b[6n` → `\x1b[row;colR`). Both `cmd.exe` and `powershell.exe` send this on startup. Without forwarding the response, the shell hangs indefinitely waiting for the reply.

Fix: `EventProxy` holds a clone of the PTY writer `UnboundedSender<Vec<u8>>`. In `send_event`, `Event::PtyWrite(s)` bytes are sent through that channel to the PTY writer task, which forwards them to the shell's stdin.

```rust
impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        if let Event::PtyWrite(s) = event {
            let _ = self.pty_tx.send(s.into_bytes());
        }
    }
}
```

The PTY writer channel must be created before `TerminalState` so the sender can be given to `EventProxy` at construction time.

### HiDPI / DPI scaling
`window.inner_size()` returns physical pixels. On a 2× HiDPI screen, a 1200×800 logical window is 2400×1600 physical. Without scaling:
- The terminal grid was 266×88 cells (physical / 9px) instead of 133×44 (logical / 9px) — too many cells, all blank
- `TextArea.scale = 1.0` rendered 14px-high glyphs at 7pt physical — invisible

Fixes applied in `app.rs` and `renderer.rs`:
```rust
// app.rs — cols/rows from logical pixels
let scale = window.scale_factor() as f32;
let cols = (size.width as f32 / (cw * scale)).floor().max(1.0) as u16;
let rows = (size.height as f32 / (ch * scale)).floor().max(1.0) as u16;

// renderer.rs — scale font metrics and TextArea
Metrics::new(FONT_SIZE * scale_factor, LINE_HEIGHT * scale_factor)
TextArea { scale: self.scale_factor, ... }
```

### alacritty_terminal 0.24 API notes
- `SizeInfo` was removed. Use `alacritty_terminal::term::test::TermSize` (public despite being in `mod test`).
- `Term::new(Config::default(), &TermSize { columns, screen_lines }, EventProxy)`
- `term.renderable_content()` returns `RenderableContent` with `display_iter` and `cursor`.
- `Indexed<Cell>` — access via `.point.line.0`, `.point.column.0`, `.cell.c`, `.cell.fg`, `.cell.bg`.

### wgpu 23 + glyphon 0.7 + cosmic-text 0.12
- glyphon 0.7 resolves to cosmic-text 0.12.1 (not 0.14). The `set_rich_text` API in 0.12 takes 4 args (no `alignment` param) and `Attrs` (not `&Attrs`).
- `enumerate_adapters` in wgpu 23 returns `Vec<Adapter>` directly.
- AMD iGPU: `request_adapter` with `PowerPreference::None` picks the surface-compatible adapter.

### Font loading on Windows
`Family::Monospace` resolves to nothing on Windows without an explicit `set_monospace_family()` call. Fix: load Cascadia Code / Consolas / Lucida Console from `C:\Windows\Fonts\` and call `font_system.db_mut().set_monospace_family(name)`.

### Colour rendering
Cells are grouped into same-colour runs and passed to `Buffer::set_rich_text` as `(str, Attrs::color)` spans. This avoids one TextArea per cell while keeping full per-cell colour accuracy.

### Cursor rendering
The cursor cell is rendered as `█` (U+2588 FULL BLOCK) in Gruvbox foreground colour `rgb(235, 219, 178)`. This keeps the cursor visible even when the underlying cell character is a space, without requiring a separate background-rect draw pass.

### wgpu clear colour
wgpu clear colours are in **linear** space, not sRGB. Gruvbox hard-dark background `#1d2021` (sRGB ~11%) linearises to ~0.010. Using `r: 0.010, g: 0.010, b: 0.010` gives the correct near-black background.

---

## How to Run

```
cargo run --bin alfred-app
```

On Windows, RUST_LOG environment variables must be set separately in PowerShell:
```powershell
$env:RUST_LOG = "info"; cargo run --bin alfred-app
$env:RUST_LOG = "debug"; cargo run --bin alfred-app
```

---

## Known Limitations (to fix in later phases)

- **No scrollback UI** — alacritty_terminal tracks scrollback but there is no scroll wheel handling yet.
- **No cell background colours** — `TermCell.bg` is stored but the renderer only uses foreground colour. Background rects planned for Phase 2.
- **No bold/italic** — cell `Flags` (bold, italic, underline) are not yet mapped to glyphon `Attrs`.
- **HiDPI cell sizing** — cell dimensions (9×18px logical) are hardcoded. Should derive from actual font metrics at the given scale factor.
- **AMD iGPU selection** — explicit name-based adapter selection (to work around potential `DiscreteGpu` misreport) not yet added.
- **Bundled font** — uses system font discovery. A bundled Cascadia Code / JetBrains Mono fallback should be added for portability.
- **alfred-cli IPC** — CLI stub only; named pipe / Unix socket wiring is Phase 5.
