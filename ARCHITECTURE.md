# Alfred — Architecture & Technical Design

> A GPU-accelerated terminal multiplexer for Windows and Linux, built in Rust.
> Inspired by [cmux](https://github.com/manaflow-ai/cmux) for macOS.

---

## Table of Contents

1. [Project Goals](#1-project-goals)
2. [Hardware Context](#2-hardware-context)
3. [Why Rust](#3-why-rust)
4. [Technology Stack](#4-technology-stack)
5. [Crate Selection & Rationale](#5-crate-selection--rationale)
6. [Architecture Overview](#6-architecture-overview)
7. [Component Design](#7-component-design)
8. [Rendering Pipeline](#8-rendering-pipeline)
9. [PTY & Shell Integration](#9-pty--shell-integration)
10. [Multiplexing & Layout](#10-multiplexing--layout)
11. [AI Agent Notification System](#11-ai-agent-notification-system)
12. [Sidebar & Project Management](#12-sidebar--project-management)
13. [IPC & CLI Control](#13-ipc--cli-control)
14. [Cross-Platform Strategy](#14-cross-platform-strategy)
15. [Build Phases](#15-build-phases)
16. [What We Don't Reuse from Ghostty (and Why)](#16-what-we-dont-reuse-from-ghostty-and-why)
17. [Performance Targets](#17-performance-targets)
18. [Directory Structure](#18-directory-structure)

---

## 1. Project Goals

Alfred aims to deliver the cmux experience on Windows and Linux:

| Feature | Description |
|---|---|
| Fast terminal multiplexer | Multiple tabs + split panes in a single GPU-rendered window |
| AI agent awareness | Visual notifications when Claude Code / Codex / other agents need attention |
| Sidebar | Switch between projects, see git branch, working directory, listening ports |
| CLI control | `alfred` CLI tool for scripting workspace creation, pane splits, key sending |
| Cross-platform | Windows 10+ and Linux (same codebase) |
| Performance | < 1ms input latency, GPU-accelerated rendering, no Electron overhead |

**Not in scope initially:** Built-in browser, remote daemon.

---

## 2. Hardware Context

**Development machine GPU:** AMD Radeon(TM) Graphics (integrated), 512MB VRAM
**Driver:** 30.0.13044.3001

This is an AMD integrated GPU (RDNA/GCN family). Key implications:

- **Use DX12 as the primary backend on Windows** (not Vulkan). AMD iGPU + wgpu + DX12 is more stable than the Vulkan WSI path on Windows. The wgpu team explicitly recommends this.
- **AMD iGPU misreport bug:** AMD integrated GPUs are incorrectly labeled as `DiscreteGpu` in wgpu's DX12 backend. When selecting a GPU adapter, enumerate by name rather than relying on `DeviceType`.
- **Bundle DXC (`dxcompiler.dll`):** DirectX Shader Compiler v1.8.2502+ produces faster shaders than the legacy FXC compiler. Ship it in the release binary.
- **512MB VRAM is sufficient** for a terminal — glyph texture atlases are typically 1–4MB.
- **Linux:** Use Vulkan backend (AMD has excellent Vulkan support via RADV/AMDVLK).

---

## 3. Why Rust

- **No garbage collector** — critical for < 1ms keyboard input latency on hot paths
- **Cross-platform:** single codebase compiles natively to Windows and Linux
- **Proven in terminals:** Alacritty, WezTerm, Rio, Zellij are all production Rust terminals
- **wgpu:** The best cross-platform GPU abstraction exists in the Rust ecosystem
- **Memory safety** without the complexity of C++
- **async/await + tokio** for clean PTY I/O, IPC, and git query concurrency

---

## 4. Technology Stack

```
┌─────────────────────────────────────────────────────────────────┐
│                           Alfred                                │
├──────────────┬──────────────────┬──────────────────────────────┤
│  UI Layer    │  Terminal Model  │  GPU Renderer                 │
│              │                  │                               │
│  winit       │  alacritty_      │  wgpu 28+                     │
│  (windowing) │  terminal 0.24   │  ├─ DX12     (Windows)       │
│              │                  │  └─ Vulkan   (Linux)          │
│  Custom      │  vte 0.15        │                               │
│  immediate   │  (VT parser)     │  glyphon                      │
│  mode UI     │                  │  + cosmic-text                │
│              │                  │  + swash                      │
│              │                  │  (text shaping + atlas)       │
├──────────────┴──────────────────┴──────────────────────────────┤
│                      PTY Layer                                  │
│  portable-pty 0.9 (patched) — ConPTY on Windows, Unix on Linux │
├─────────────────────────────────────────────────────────────────┤
│                   Async Runtime                                 │
│  tokio — PTY I/O, IPC server, git queries, notification events │
└─────────────────────────────────────────────────────────────────┘
```

---

## 5. Crate Selection & Rationale

### Terminal Emulation

**`alacritty_terminal` v0.24** — [crates.io](https://crates.io/crates/alacritty_terminal)

The cleanest standalone terminal model in the Rust ecosystem. Apache 2.0. No GUI dependency. You feed it bytes via `process_bytes()`; it maintains the VT grid, colors, cursor, scrollback, hyperlinks, selection. Iterate visible cells with `renderable_content()`.

Compared to alternatives:
- vs `wezterm-term`: heavier, pulls more dependencies, but has sixel/iTerm2 images. Switch if those are needed later.
- vs raw `vte`: `vte` only parses — no grid state. `alacritty_terminal` builds on `vte` and adds the grid.

**`vte` v0.15** — [crates.io](https://crates.io/crates/vte)

The underlying VT escape sequence parser (Paul Williams state machine). Used directly by `alacritty_terminal`. Fastest VT parser in the Rust ecosystem. We implement the `Perform` trait for OSC sequence detection (needed for AI agent notifications).

### PTY (Pseudo-Terminal)

**`portable-pty` v0.9** — [crates.io](https://crates.io/crates/portable-pty)

Single API for ConPTY (Windows) and Unix PTY (Linux). Abstracts `CreatePseudoConsole` on Windows and `openpty/posix_openpt` on Linux.

**Known issue:** Does not pass modern ConPTY creation flags:
- `PSEUDOCONSOLE_WIN32_INPUT_MODE` — needed for correct key event handling
- `PSEUDOCONSOLE_PASSTHROUGH_MODE` — direct VT relay (Windows 11 22H2+)
- `PSEUDOCONSOLE_RESIZE_QUIRK` — fixes resize artifacts

**Plan:** Fork `portable-pty` locally and add these flags. Small patch (~20 lines).

### GPU Rendering

**`wgpu` v28** — [crates.io](https://crates.io/crates/wgpu)

Cross-platform GPU abstraction over DX12/Vulkan/Metal/OpenGL. Idiomatic safe Rust. Used by Firefox, Rio terminal, and many games.

Configuration for Alfred:
```rust
// Windows — explicit DX12 for AMD stability
wgpu::Backends::DX12 + Dx12Compiler::Auto  // prefers DXC over FXC if available

// Linux
wgpu::Backends::VULKAN
```

**`glyphon`** — [crates.io](https://crates.io/crates/glyphon)

GPU text renderer that integrates as middleware into an existing wgpu render pass. Uses `cosmic-text` for text shaping and `etagere` for shelf-pack glyph atlas allocation. Handles ligatures, emoji, BiDi, color fonts. No separate render pass needed.

**`cosmic-text`** — [crates.io](https://crates.io/crates/cosmic_text)

Pure-Rust multi-line text shaping and layout. Uses `rustybuzz` (HarfBuzz port) for shaping and `swash` for rasterization. Maintained by System76. No C dependencies — simpler build than bundling HarfBuzz.

### Windowing

**`winit`** — [crates.io](https://crates.io/crates/winit)

The standard cross-platform windowing crate. Handles window creation, event loop, keyboard/mouse input, resize on both Windows and Linux (X11/Wayland). Works with wgpu via `wgpu::Surface`.

### Async Runtime

**`tokio`** — PTY I/O runs in tokio tasks. Each pane has its own tokio task reading PTY output and writing to a channel. IPC server is a tokio `NamedPipe` (Windows) / `UnixListener` (Linux). Git queries run in `tokio::task::spawn_blocking`.

### Config & Serialization

**`toml` + `serde`** — Config file at `~/.config/alfred/config.toml` (Linux) / `%APPDATA%\alfred\config.toml` (Windows).

### Git Integration

**`git2`** — libgit2 bindings for reading branch name, dirty status, remote URL for the sidebar. Runs in `spawn_blocking` to avoid blocking the render thread.

### CLI

**`clap`** — Argument parsing for the `alfred` CLI binary. IPC transport: named pipe on Windows, Unix domain socket on Linux.

---

## 6. Architecture Overview

Alfred is a **Cargo workspace** with two binaries:

```
alfred/
├── crates/
│   ├── alfred-app/     ← main terminal application binary
│   └── alfred-cli/     ← alfred CLI tool
└── Cargo.toml
```

### Threading Model

```
Main Thread (winit event loop)
  │
  ├── Render Thread
  │     wgpu render pass → glyphon text → swapchain present
  │
  ├── Pane Tasks (one per pane, tokio)
  │     PTY reader → vte parser → terminal grid update → dirty flag
  │
  ├── IPC Server Task (tokio)
  │     Named pipe / Unix socket → command dispatch
  │
  └── Git Query Tasks (tokio spawn_blocking)
        git2 → branch/status → sidebar state
```

### State Model

```
AppState
  └── WorkspaceManager
        └── Vec<Workspace>
              ├── name: String
              ├── layout: PaneTree (binary tree)
              │     └── leaf: Pane
              │           ├── TerminalModel (alacritty_terminal::Term)
              │           ├── PtyHandle (portable-pty)
              │           ├── NotificationState
              │           └── dirty: AtomicBool
              └── git_info: GitInfo (branch, status)
```

---

## 7. Component Design

### `crates/alfred-app/src/`

```
main.rs                  — winit event loop, wgpu init, top-level dispatch
app.rs                   — AppState, frame render coordination

terminal/
  mod.rs
  pty.rs                 — spawn shell via portable-pty, read/write tasks
  emulator.rs            — wrap alacritty_terminal::Term, feed PTY bytes
  renderer.rs            — custom wgpu widget: map terminal grid → glyphon text
  osc.rs                 — OSC 9/99/777 parser on top of vte::Perform

workspace/
  mod.rs
  manager.rs             — WorkspaceManager: create/delete/switch workspaces
  pane.rs                — Pane: owns TerminalModel + PtyHandle
  layout.rs              — PaneTree: binary tree for splits, resize math

ui/
  mod.rs
  sidebar.rs             — left sidebar: workspace list, git info, ports, notifs
  tabbar.rs              — top tab bar per workspace
  notification.rs        — notification overlay / badge rendering
  theme.rs               — color themes (reads config)

ipc/
  server.rs              — named pipe (Windows) / Unix socket (Linux) server
  commands.rs            — command enum: WorkspaceCreate, PaneSplit, SendKeys...

config/
  mod.rs                 — load/save ~/.config/alfred/config.toml
  types.rs               — Config struct: font, theme, keybindings, shell

git/
  mod.rs                 — git2 queries: branch, dirty, remote, PR detection

platform/
  mod.rs
  windows.rs             — ConPTY flags, named pipe, DX12 adapter selection
  linux.rs               — Unix PTY, Unix socket, Vulkan adapter selection
```

### `crates/alfred-cli/src/`

```
main.rs                  — clap CLI: connect to IPC, send commands, print responses
```

---

## 8. Rendering Pipeline

### Per Frame

```
1. winit::Event::RedrawRequested
2. Collect dirty panes (AtomicBool changed since last frame)
3. For each dirty pane:
   a. Lock terminal grid (alacritty_terminal::Term)
   b. Iterate renderable_content() → cells (char, fg, bg, attrs)
   c. Build glyphon TextArea for this pane's viewport
   d. Update glyph atlas if new glyphs seen
4. Begin wgpu render pass
5. Clear background (theme background color)
6. Draw pane backgrounds + split borders
7. glyphon::TextRenderer::render() — all pane text in one render pass
8. Draw UI chrome: sidebar, tab bar, notification badges
9. wgpu::SurfaceTexture::present()
```

### Glyph Atlas

Managed by `glyphon`. Shelf-pack allocator (`etagere`) bins glyphs by height. Atlas starts at 512×512, doubles when full up to 4096×4096. At 512MB VRAM, a 1024×1024 RGBA8 atlas uses 4MB — trivial.

### Font Rendering

```
cosmic-text FontSystem
  └── font discovery (system fonts + bundled fallback)
        ├── rustybuzz shaping (ligatures, kerning, BiDi)
        └── swash rasterization (subpixel, hinting)
```

A monospace font (JetBrains Mono or Cascadia Code) is bundled as a fallback so Alfred works without system font installation.

---

## 9. PTY & Shell Integration

### Windows (ConPTY)

```
CreatePseudoConsole(size, input_pipe, output_pipe, flags)
  flags: PSEUDOCONSOLE_WIN32_INPUT_MODE
       | PSEUDOCONSOLE_PASSTHROUGH_MODE  (Win11 22H2+)
       | PSEUDOCONSOLE_RESIZE_QUIRK

Default shell: %COMSPEC% → PowerShell → Git Bash → WSL (in order)
```

### Linux (Unix PTY)

```
openpty() → master_fd + slave_fd
fork() + setsid() + dup2(slave_fd, 0/1/2) + exec(shell)
Default shell: $SHELL or /bin/bash
```

### PTY I/O Task (per pane)

```rust
async fn pty_reader_task(
    reader: Box<dyn Read + Send>,
    tx: mpsc::Sender<Vec<u8>>,
) {
    let mut buf = [0u8; 8192];
    loop {
        let n = reader.read(&mut buf).await?;
        tx.send(buf[..n].to_vec()).await?;
    }
}

fn process_pty_output(pane: &mut Pane, bytes: &[u8]) {
    pane.terminal.process_bytes(bytes);
    pane.dirty.store(true, Ordering::Release);
}
```

---

## 10. Multiplexing & Layout

### Pane Tree (Binary Split Tree)

```rust
enum PaneTree {
    Split {
        direction: Direction,  // Horizontal | Vertical
        ratio: f32,            // 0.0..1.0 where the split falls
        left: Box<PaneTree>,
        right: Box<PaneTree>,
    },
    Leaf(PaneId),
}
```

Resize: walk the tree top-down, passing available rect. Each split divides the rect by `ratio`.

### Keyboard Shortcuts (default)

| Action | Shortcut |
|---|---|
| Split vertical | `Ctrl+Shift+E` |
| Split horizontal | `Ctrl+Shift+O` |
| Focus next pane | `Ctrl+Shift+]` |
| Focus prev pane | `Ctrl+Shift+[` |
| New tab | `Ctrl+Shift+T` |
| Close pane | `Ctrl+Shift+W` |
| Next tab | `Ctrl+Tab` |
| Prev tab | `Ctrl+Shift+Tab` |

All keybindings configurable via `config.toml`.

---

## 11. AI Agent Notification System

This is the core feature that makes Alfred different from a generic terminal multiplexer.

### OSC Escape Sequences

AI agents (Claude Code, Codex, etc.) emit OSC sequences to signal state:

| Sequence | Meaning | Action |
|---|---|---|
| `OSC 777 ; notify ; <title> ; <body> ST` | Agent needs attention | Badge on pane + tab |
| `OSC 9 ; <message> ST` | Notification | Log to notification panel |
| `OSC 99 ; <data> ST` | Custom agent data | Parse and route |

### Implementation

`vte`'s `Perform` trait `osc_dispatch()` fires for every OSC sequence. We intercept this in `osc.rs`:

```rust
impl vte::Perform for OscInterceptor {
    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        match params {
            [b"777", b"notify", title, body] => {
                self.events.send(NotificationEvent {
                    pane_id: self.pane_id,
                    title: String::from_utf8_lossy(title).into(),
                    body: String::from_utf8_lossy(body).into(),
                });
            }
            [b"9", msg] => { /* log notification */ }
            _ => {}
        }
        // all bytes also forwarded to alacritty_terminal
    }
}
```

### Visual Indicators

When a pane has an unacknowledged notification:
- Pane border renders in accent color (blue ring, matching cmux style)
- Tab shows a colored dot badge
- Notification entry appears in the sidebar

Notifications clear when the pane receives focus.

---

## 12. Sidebar & Project Management

Fixed-width collapsible panel on the left containing:

### Workspace List
- Each workspace shows: icon, name, notification badge count, git branch + dirty indicator
- Click to switch workspace
- Right-click for rename/close

### Per-Workspace Info (expanded)
- Working directory path
- Git branch + ahead/behind remote count
- Linked PR number (detected from branch name pattern + remote URL)
- Listening ports (via `netstat` parsing or OS API)

### Notification Log
- Chronological list of agent notifications across all panes
- Click to focus the originating pane

---

## 13. IPC & CLI Control

### Server

On startup, Alfred creates an IPC server:
- **Windows:** Named pipe at `\\.\pipe\alfred`
- **Linux:** Unix domain socket at `/tmp/alfred.sock`

Commands are JSON newline-delimited:

```json
{"cmd": "workspace.create", "name": "my-project", "dir": "/path/to/project"}
{"cmd": "workspace.list"}
{"cmd": "pane.split", "workspace_id": "abc", "direction": "vertical"}
{"cmd": "pane.send_keys", "pane_id": "xyz", "keys": "claude\n"}
{"cmd": "notification.list"}
```

### CLI Tool (`alfred`)

```bash
alfred workspace create --name "my-project" --dir ./project
alfred workspace list
alfred pane split --direction vertical
alfred pane send-keys "claude --model claude-opus-4-6\n"
alfred notification list
```

---

## 14. Cross-Platform Strategy

```rust
#[cfg(target_os = "windows")]
mod platform {
    pub fn gpu_backends() -> wgpu::Backends { wgpu::Backends::DX12 }
    pub fn ipc_path() -> String { r"\\.\pipe\alfred".into() }
    pub fn default_shell() -> PathBuf { /* %COMSPEC% detection */ }
}

#[cfg(target_os = "linux")]
mod platform {
    pub fn gpu_backends() -> wgpu::Backends { wgpu::Backends::VULKAN }
    pub fn ipc_path() -> String { "/tmp/alfred.sock".into() }
    pub fn default_shell() -> PathBuf { /* $SHELL detection */ }
}
```

### Config Paths

| Platform | Config | State |
|---|---|---|
| Windows | `%APPDATA%\alfred\config.toml` | `%APPDATA%\alfred\sessions\` |
| Linux | `~/.config/alfred/config.toml` | `~/.local/share/alfred/sessions/` |

---

## 15. Build Phases

### Phase 1 — Core Terminal

- [ ] Cargo workspace: `alfred-app` + `alfred-cli` crates
- [ ] `winit` event loop + `wgpu` surface (DX12 on Windows, Vulkan on Linux)
- [ ] `portable-pty` (patched) spawning a shell
- [ ] `alacritty_terminal` fed PTY bytes via tokio task
- [ ] Custom wgpu render widget: terminal grid → `glyphon` text
- [ ] Keyboard input: winit key events → PTY write
- [ ] Resize: winit resize → ConPTY resize → terminal model resize
- [ ] Cursor rendering (blinking block)
- [ ] 256-color + truecolor support
- [ ] Scrollback

**Success criterion:** `cargo run` opens a window with a working shell.

### Phase 2 — Multiplexing

- [ ] Horizontal + vertical split panes (binary tree layout)
- [ ] Tab bar (create, switch, close, rename)
- [ ] Focus management (keyboard shortcuts)
- [ ] Pane resize (drag divider)
- [ ] Session persistence (save/restore on exit/start)

**Success criterion:** Multiple shells in split panes, tab switching works.

### Phase 3 — Sidebar

- [ ] Collapsible left sidebar
- [ ] Workspace list with names
- [ ] Git branch + dirty status per workspace (`git2`)
- [ ] Working directory display
- [ ] Listening ports

**Success criterion:** Sidebar shows correct git info, updates on branch switch.

### Phase 4 — AI Agent Notifications

- [ ] OSC 777/9/99 detection via `vte` `osc_dispatch`
- [ ] Per-pane notification state
- [ ] Pane border accent color on notification
- [ ] Tab badge dot
- [ ] Notification log in sidebar
- [ ] Clear on focus

**Success criterion:** `echo -e "\e]777;notify;Claude;Ready\a"` triggers visual indicator.

### Phase 5 — IPC & CLI

- [ ] Named pipe (Windows) / Unix socket (Linux) server in tokio
- [ ] JSON command protocol
- [ ] `alfred-cli`: workspace, pane, notification commands

**Success criterion:** `alfred pane split` from another terminal splits the active workspace.

---

## 16. What We Don't Reuse from Ghostty (and Why)

Ghostty is written in Zig with a C ABI (`libghostty`). Rust FFI to C is possible, but:

| Issue | Detail |
|---|---|
| Windows build broken | `libghostty` fails to build on Windows due to `libxml2` issues (open bug #11697) |
| Requires Zig toolchain | Build-time `zig` compiler dependency — non-standard, complex CI |
| API is unstable | Pre-1.0, frequent breaking changes |
| No DX12 rendering path | Ghostty renders via Metal (macOS) and OpenGL (Linux) — no DX12 |
| Pure Rust alternatives exist | `alacritty_terminal` + `wgpu` + `glyphon` is equivalent quality |

**We take inspiration from Ghostty:** OSC notification sequences, visual notification ring UI, keyboard shortcut philosophy.

**Revisit libghostty when:** 1.0 releases, Windows build works, API stabilizes.

---

## 17. Performance Targets

| Metric | Target | How |
|---|---|---|
| Input latency | < 1ms key → PTY write | Synchronous write on winit key event |
| Render throughput | 60+ FPS stable | wgpu DX12, dirty-only cell updates |
| Large scroll | < 16ms per frame | glyphon atlas caches glyphs |
| PTY throughput | > 50MB/s | 8KB read buffer, tokio async I/O |
| Memory (idle) | < 50MB | No Electron/Chromium overhead |
| Startup | < 500ms | Minimal init, lazy font loading |

---

## 18. Directory Structure

```
alfred/
├── Cargo.toml                        # Workspace manifest
├── ARCHITECTURE.md                   # This document
├── README.md                         # User-facing docs
├── .github/
│   └── workflows/
│       ├── build-windows.yml
│       └── build-linux.yml
│
├── crates/
│   ├── alfred-app/
│   │   ├── Cargo.toml
│   │   ├── build.rs                  # Copy DXC dll, embed Windows manifest
│   │   └── src/
│   │       ├── main.rs
│   │       ├── app.rs
│   │       ├── terminal/
│   │       │   ├── mod.rs
│   │       │   ├── pty.rs
│   │       │   ├── emulator.rs
│   │       │   ├── renderer.rs
│   │       │   └── osc.rs
│   │       ├── workspace/
│   │       │   ├── mod.rs
│   │       │   ├── manager.rs
│   │       │   ├── pane.rs
│   │       │   └── layout.rs
│   │       ├── ui/
│   │       │   ├── mod.rs
│   │       │   ├── sidebar.rs
│   │       │   ├── tabbar.rs
│   │       │   ├── notification.rs
│   │       │   └── theme.rs
│   │       ├── ipc/
│   │       │   ├── server.rs
│   │       │   └── commands.rs
│   │       ├── config/
│   │       │   ├── mod.rs
│   │       │   └── types.rs
│   │       ├── git/
│   │       │   └── mod.rs
│   │       └── platform/
│   │           ├── mod.rs
│   │           ├── windows.rs
│   │           └── linux.rs
│   │
│   └── alfred-cli/
│       ├── Cargo.toml
│       └── src/
│           └── main.rs
│
├── assets/
│   ├── fonts/
│   │   └── CascadiaCode.ttf          # Bundled fallback monospace font
│   └── icons/
│       └── alfred.ico
│
└── vendor/
    └── portable-pty/                 # Patched fork with ConPTY flags
```

---

## Key Dependencies (Cargo.toml preview)

```toml
[workspace]
members = ["crates/alfred-app", "crates/alfred-cli"]

# Shared dependencies in workspace Cargo.toml
[workspace.dependencies]
tokio    = { version = "1",    features = ["full"] }
serde    = { version = "1",    features = ["derive"] }

# alfred-app specific
[dependencies]
winit             = "0.30"
wgpu              = { version = "28", features = ["dx12", "vulkan"] }
glyphon           = "0.7"
cosmic-text       = "0.14"
alacritty_terminal = "0.24"
vte               = "0.15"
portable-pty      = { path = "../../vendor/portable-pty" }
tokio             = { workspace = true }
git2              = "0.20"
serde             = { workspace = true }
toml              = "0.8"
clap              = { version = "4", features = ["derive"] }

[target.'cfg(windows)'.dependencies]
windows = { version = "0.58", features = [
  "Win32_System_Console",
  "Win32_System_Pipes",
  "Win32_NetworkManagement_IpHelper",
] }

[target.'cfg(unix)'.dependencies]
nix = { version = "0.29", features = ["term", "pty"] }

# alfred-cli specific
[dependencies]
clap   = { version = "4", features = ["derive"] }
serde  = { workspace = true }
serde_json = "1"
tokio  = { workspace = true }
```

---

*Architecture as of March 2026. Update as implementation evolves.*
