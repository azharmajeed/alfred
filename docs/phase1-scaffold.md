# Phase 1 — Core Terminal Scaffold

**Date:** 2026-03-23
**Result:** `cargo build` compiles with zero warnings. `cargo run --bin alfred-app` opens a GPU-accelerated terminal window.

---

## What Was Built

A fully functional single-pane terminal window:

| Feature | Status |
|---|---|
| Cargo workspace (`alfred-app` + `alfred-cli`) | ✅ |
| winit window (1200×800 default) | ✅ |
| wgpu 23 surface — DX12 on Windows, Vulkan on Linux | ✅ |
| PTY spawn (portable-pty 0.9) — pwsh/cmd on Windows | ✅ |
| alacritty_terminal 0.24 VT grid | ✅ |
| glyphon 0.7 GPU text rendering | ✅ |
| Keyboard input → PTY (full key table) | ✅ |
| Window resize → PTY + terminal grid resize | ✅ |
| Cursor position tracking | ✅ |
| 256-colour + truecolor support | ✅ |
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
```

### alacritty_terminal 0.24 API notes
- `SizeInfo` was removed. Use `alacritty_terminal::term::test::TermSize` (public despite being in `mod test`).
- `Term::new(Config::default(), &TermSize { columns, screen_lines }, EventProxy)`
- `term.renderable_content()` returns `RenderableContent` with `display_iter` and `cursor`.
- `Indexed<Cell>` — access via `.point.line.0`, `.point.column.0`, `.cell.c`, `.cell.fg`, `.cell.bg`.

### wgpu 23 + glyphon 0.7 + cosmic-text 0.12
- glyphon 0.7 resolves to cosmic-text 0.12.1 (not 0.14). The `set_rich_text` API in 0.12 takes 4 args (no `alignment` param) and `Attrs` (not `&Attrs`).
- `enumerate_adapters` in wgpu 23 returns `Vec<Adapter>` directly.
- AMD iGPU note: we log all adapter candidates; `request_adapter` picks the one compatible with the surface. If the wrong GPU is selected, we can add explicit name-based selection later.

### Colour rendering
Cells are grouped into same-colour runs and passed to `Buffer::set_rich_text` as `(str, Attrs::color)` spans. This avoids one TextArea per cell while keeping full per-cell colour accuracy.

---

## How to Run

```bash
cargo run --bin alfred-app
# or
cargo run --bin alfred -- workspace list
```

### Environment variables
```bash
RUST_LOG=info cargo run --bin alfred-app    # GPU adapter info
RUST_LOG=debug cargo run --bin alfred-app   # all GPU candidates
```

---

## Known Limitations (to fix in later phases)

- **No scrollback UI** — alacritty_terminal tracks scrollback but there's no scroll wheel handling yet.
- **No cell background colours** — `TermCell.bg` is stored but the renderer only uses foreground colour. Add background rects in Phase 2.
- **No bold/italic** — cell `Flags` (bold, italic, underline) are not yet mapped to glyphon `Attrs`.
- **AMD iGPU selection** — `request_adapter` with `PowerPreference::None` may pick the correct adapter but explicit name-based selection (to work around the `DiscreteGpu` misreport) should be added.
- **ConPTY flags** — `portable-pty` uses default ConPTY flags. The patched fork with `PSEUDOCONSOLE_WIN32_INPUT_MODE | PSEUDOCONSOLE_PASSTHROUGH_MODE` is planned for Phase 1.5.
- **Bundled font** — currently uses system font discovery. A bundled Cascadia Code / JetBrains Mono fallback should be added for portability.
- **alfred-cli IPC** — CLI stub only; named pipe / Unix socket wiring is Phase 5.
